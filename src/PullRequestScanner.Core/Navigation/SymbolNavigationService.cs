using System.Text.RegularExpressions;
using PullRequestScanner.Core.Exceptions;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Navigation;

public sealed record SymbolCandidate(
    string Name,
    string DisplayName,
    bool IsLikelyExternal,
    int ColumnIndex);

public static class SymbolCandidateExtractor
{
    private static readonly Regex CallRegex = new(
        @"(?<![\w.])(?<qualified>(?:[A-Za-z_]\w*(?:::|\.))*[A-Za-z_]\w*)\s*\(",
        RegexOptions.CultureInvariant);

    internal static readonly HashSet<string> Keywords = new(StringComparer.OrdinalIgnoreCase)
    {
        "if", "for", "foreach", "while", "switch", "catch", "using", "new",
        "typeof", "sizeof", "nameof", "lock", "return", "else", "with", "assert"
    };

    private static readonly HashSet<string> ExternalRoots = new(StringComparer.OrdinalIgnoreCase)
    {
        "std",
        "__std",
        "System"
    };

    public static IReadOnlyList<SymbolCandidate> ExtractCandidates(string code)
    {
        var values = new List<SymbolCandidate>();
        foreach (Match match in CallRegex.Matches(StripCommentsAndStrings(code)))
        {
            string displayName = match.Groups["qualified"].Value;
            string symbol = GetSimpleName(displayName);
            if (Keywords.Contains(symbol) ||
                values.Any(value => value.DisplayName.Equals(
                    displayName,
                    StringComparison.Ordinal)))
            {
                continue;
            }

            string root = displayName
                .Split(["::", "."], StringSplitOptions.RemoveEmptyEntries)[0];
            values.Add(new SymbolCandidate(
                symbol,
                displayName,
                ExternalRoots.Contains(root),
                match.Groups["qualified"].Index));
        }

        return values;
    }

    public static IReadOnlyList<string> Extract(string code) =>
        ExtractCandidates(code)
            .Select(candidate => candidate.Name)
            .ToArray();

    private static string GetSimpleName(string displayName)
    {
        int colonSeparator = displayName.LastIndexOf("::", StringComparison.Ordinal);
        int dotSeparator = displayName.LastIndexOf('.');
        return colonSeparator > dotSeparator && colonSeparator >= 0
            ? displayName[(colonSeparator + 2)..]
            : dotSeparator >= 0
                ? displayName[(dotSeparator + 1)..]
                : displayName;
    }

    private static string StripCommentsAndStrings(string code)
    {
        char[] text = code.ToCharArray();
        bool inString = false;
        bool inCharacter = false;
        bool escaped = false;
        for (int index = 0; index < text.Length; index++)
        {
            char current = text[index];
            if (inString || inCharacter)
            {
                if (escaped)
                {
                    escaped = false;
                }
                else if (current == '\\')
                {
                    escaped = true;
                }
                else if (inString && current == '"')
                {
                    inString = false;
                }
                else if (inCharacter && current == '\'')
                {
                    inCharacter = false;
                }

                text[index] = ' ';
                continue;
            }

            if (current == '"')
            {
                inString = true;
                text[index] = ' ';
                continue;
            }

            if (current == '\'')
            {
                inCharacter = true;
                text[index] = ' ';
                continue;
            }

            if (current == '/' &&
                index + 1 < text.Length &&
                text[index + 1] == '/')
            {
                Array.Fill(text, ' ', index, text.Length - index);
                break;
            }

            if (current == '/' &&
                index + 1 < text.Length &&
                text[index + 1] == '*')
            {
                int end = code.IndexOf("*/", index + 2, StringComparison.Ordinal);
                int length = end >= 0 ? end + 2 - index : text.Length - index;
                Array.Fill(text, ' ', index, length);
                index += length - 1;
            }
        }

        return new string(text);
    }
}

public sealed class SymbolNavigationService(ProcessRunner processRunner)
{
    public async Task<SymbolLocation?> FindDefinitionAsync(
        ReviewSessionState state,
        string symbol,
        string preferredPath,
        string? commit = null,
        int? sourceLineNumber = null,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(state.LocalRepositoryPath))
        {
            throw new ReviewOperationException("The pull request repository is not available.");
        }

        List<SymbolLocation> candidates = [];
        commit ??= state.Details.HeadCommitSha;
        IReadOnlyList<string> extensionPathSpecs = GetExtensionPathSpecs(preferredPath);
        string directory = Path.GetDirectoryName(preferredPath.Replace('\\', '/'))?.Replace('\\', '/') ?? string.Empty;

        await AddMatchesAsync(
            candidates,
            state,
            symbol,
            commit,
            [preferredPath.Replace('\\', '/')],
            cancellationToken);

        if (!HasStrongDefinition(candidates, preferredPath, sourceLineNumber) &&
            !string.IsNullOrWhiteSpace(directory))
        {
            await AddMatchesAsync(
                candidates,
                state,
                symbol,
                commit,
                [$":(glob){directory}/**/*"],
                cancellationToken);
        }

        if (!HasStrongDefinition(candidates, preferredPath, sourceLineNumber))
        {
            await AddMatchesAsync(
                candidates,
                state,
                symbol,
                commit,
                extensionPathSpecs,
                cancellationToken);
        }

        return candidates
            .Select(candidate => Score(candidate, preferredPath))
            .Where(candidate => candidate.Score >= 120)
            .Where(candidate =>
                sourceLineNumber is null ||
                !candidate.FilePath.Equals(
                    preferredPath.Replace('\\', '/'),
                    StringComparison.OrdinalIgnoreCase) ||
                candidate.LineNumber != sourceLineNumber.Value)
            .OrderByDescending(candidate => candidate.Score)
            .FirstOrDefault();
    }

    private async Task AddMatchesAsync(
        ICollection<SymbolLocation> candidates,
        ReviewSessionState state,
        string symbol,
        string commit,
        IReadOnlyList<string> pathSpecs,
        CancellationToken cancellationToken)
    {
        List<string> arguments =
        [
            "-C",
            state.LocalRepositoryPath!,
            "grep",
            "--color=never",
            "--line-number",
            "--fixed-strings",
            "--word-regexp",
            symbol,
            commit,
            "--",
            .. pathSpecs
        ];

        ProcessResult result = await processRunner.RunAsync(
            "git",
            arguments,
            cancellationToken: cancellationToken);
        if (result.ExitCode is 1)
        {
            return;
        }

        if (!result.Succeeded)
        {
            throw new ReviewOperationException(
                $"Git could not search for '{symbol}' (exit {result.ExitCode}). {result.StandardError.Trim()}");
        }

        string prefix = $"{commit}:";
        foreach (string line in result.StandardOutput.Split('\n'))
        {
            string value = line.TrimEnd('\r');
            if (string.IsNullOrWhiteSpace(value))
            {
                continue;
            }

            if (value.StartsWith(prefix, StringComparison.Ordinal))
            {
                value = value[prefix.Length..];
            }

            int lineSeparator = value.IndexOf(':', StringComparison.Ordinal);
            if (lineSeparator < 0)
            {
                continue;
            }

            int numberStart = lineSeparator + 1;
            int numberSeparator = value.IndexOf(':', numberStart);
            if (numberSeparator < 0 ||
                !int.TryParse(value.AsSpan(numberStart, numberSeparator - numberStart), out int lineNumber))
            {
                continue;
            }

            string text = value[(numberSeparator + 1)..];
            SymbolLocation candidate = new(
                symbol,
                value[..lineSeparator],
                lineNumber,
                text,
                0);
            if (!candidates.Any(existing =>
                    existing.FilePath.Equals(candidate.FilePath, StringComparison.OrdinalIgnoreCase) &&
                    existing.LineNumber == candidate.LineNumber))
            {
                candidates.Add(candidate);
            }
        }
    }

    private static SymbolLocation Score(SymbolLocation location, string preferredPath)
    {
        int score = GetDefinitionScore(
            location.LineText,
            location.Symbol,
            location.FilePath);
        if (location.FilePath.Equals(
                preferredPath.Replace('\\', '/'),
                StringComparison.OrdinalIgnoreCase))
        {
            score += 20;
        }

        return location with { Score = score };
    }

    private static int GetDefinitionScore(
        string line,
        string symbol,
        string filePath)
    {
        string escaped = Regex.Escape(symbol);
        if (Regex.IsMatch(
                line,
                $@"^\s*(?:class|struct|interface|enum|record|type)\s+(?:\w+::)*{escaped}\b",
                RegexOptions.CultureInvariant) ||
            Regex.IsMatch(
                line,
                $@"^\s*(?:async\s+)?def\s+{escaped}\s*\(",
                RegexOptions.CultureInvariant) ||
            Regex.IsMatch(
                line,
                $@"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+|unsafe\s+|const\s+)*fn\s+{escaped}\s*(?:<[^>]+>)?\s*\(",
                RegexOptions.CultureInvariant) ||
            Regex.IsMatch(
                line,
                $@"^\s*func\s+(?:\([^)]*\)\s*)?{escaped}\s*\(",
                RegexOptions.CultureInvariant) ||
            Regex.IsMatch(
                line,
                $@"^\s*(?:export\s+)?(?:declare\s+)?(?:async\s+)?function\s+(?:\w+::)*{escaped}\s*(?:<[^>]+>)?\s*\(",
                RegexOptions.CultureInvariant) ||
            Regex.IsMatch(
                line,
                $@"^\s*#\s*define\s+{escaped}\s*(?:\(|\b)",
                RegexOptions.CultureInvariant))
        {
            return 160;
        }

        Match cLike = Regex.Match(
            line,
            $@"^\s*(?<prefix>(?:(?:public|private|protected|internal|static|virtual|override|abstract|async|extern|sealed|partial|final|native|constexpr|consteval|constinit|inline|explicit|friend|[\w.:<>,?*&~\[\]]+)\s+)+)(?:[\w:<>~]+\s*::\s*)?{escaped}(?:<[^;(){{}}]+>)?\s*\(",
            RegexOptions.CultureInvariant);
        if (cLike.Success && !ContainsControlKeyword(cLike.Groups["prefix"].Value))
        {
            return 160;
        }

        if (Regex.IsMatch(
                line,
                $@"^\s*(?:public|private|protected|internal|static|async|override|virtual|abstract|sealed|partial|\s)*{escaped}\s*\([^;]*\)\s*(?::[^\{{=;]+)?(?:\{{|=>)",
                RegexOptions.CultureInvariant))
        {
            return 160;
        }

        string extension = Path.GetExtension(filePath);
        bool isJavaScriptOrTypeScript = extension.Equals(
            ".js",
            StringComparison.OrdinalIgnoreCase) ||
            extension.Equals(
                ".jsx",
                StringComparison.OrdinalIgnoreCase) ||
            extension.Equals(
                ".ts",
                StringComparison.OrdinalIgnoreCase) ||
            extension.Equals(
                ".tsx",
                StringComparison.OrdinalIgnoreCase);
        if (isJavaScriptOrTypeScript && Regex.IsMatch(
                line,
                $@"^\s*(?:export\s+)?(?:const|let|var)\s+{escaped}\s*=",
                RegexOptions.CultureInvariant))
        {
            return 140;
        }

        return 0;
    }

    private static bool ContainsControlKeyword(string prefix)
    {
        string[] words = prefix.Split(
            [' ', '\t', '\r', '\n'],
            StringSplitOptions.RemoveEmptyEntries);
        return words.Any(word => SymbolCandidateExtractor.Keywords.Contains(word));
    }

    private static bool HasStrongDefinition(
        IEnumerable<SymbolLocation> candidates,
        string preferredPath,
        int? sourceLineNumber) =>
        candidates
            .Select(candidate => Score(candidate, preferredPath))
            .Any(candidate =>
                candidate.Score >= 120 &&
                (sourceLineNumber is null ||
                 !candidate.FilePath.Equals(
                     preferredPath.Replace('\\', '/'),
                     StringComparison.OrdinalIgnoreCase) ||
                 candidate.LineNumber != sourceLineNumber.Value));

    private static IReadOnlyList<string> GetExtensionPathSpecs(string path)
    {
        string extension = Path.GetExtension(path);
        return extension.ToLowerInvariant() switch
        {
            ".cs" => [":(glob)**/*.cs"],
            ".c" or ".cc" or ".cpp" or ".cxx" => [":(glob)**/*.[ch]", ":(glob)**/*.cc", ":(glob)**/*.cpp", ":(glob)**/*.cxx", ":(glob)**/*.h", ":(glob)**/*.hpp"],
            ".h" or ".hh" or ".hpp" or ".hxx" => [":(glob)**/*.h", ":(glob)**/*.hpp", ":(glob)**/*.cpp", ":(glob)**/*.cc", ":(glob)**/*.c"],
            ".js" or ".jsx" or ".ts" or ".tsx" => [":(glob)**/*.js", ":(glob)**/*.jsx", ":(glob)**/*.ts", ":(glob)**/*.tsx"],
            ".py" => [":(glob)**/*.py"],
            ".go" => [":(glob)**/*.go"],
            ".rs" => [":(glob)**/*.rs"],
            ".java" or ".kt" => [":(glob)**/*.java", ":(glob)**/*.kt"],
            _ => ["."]
        };
    }
}
