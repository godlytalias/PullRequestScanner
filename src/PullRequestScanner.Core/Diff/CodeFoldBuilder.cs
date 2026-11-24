using System.Text.RegularExpressions;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Diff;

public sealed record CodeFoldRegion(
    int StartIndex,
    int EndIndex,
    string FunctionName)
{
    public int LineCount => Math.Max(0, EndIndex - StartIndex);
}

public static class CodeFoldBuilder
{
    private static readonly Regex FunctionRegex = new(
        @"(?<name>~?[A-Za-z_]\w*(?:::[A-Za-z_]\w*)*)\s*\([^;{}]*\)\s*(?:const\b|override\b|final\b|noexcept\b|throws\b|->[^{};]+)*\s*\{$",
        RegexOptions.CultureInvariant);
    private static readonly Regex PythonFunctionRegex = new(
        @"^(?<indent>\s*)(?:async\s+)?def\s+(?<name>[A-Za-z_]\w*)\s*\(",
        RegexOptions.CultureInvariant);
    private static readonly HashSet<string> ControlNames = new(StringComparer.Ordinal)
    {
        "if", "for", "foreach", "while", "switch", "catch", "sizeof", "typeof",
        "return", "new", "assert"
    };

    public static IReadOnlyList<CodeFoldRegion> Build(
        IReadOnlyList<DiffLine> lines)
    {
        var regions = new List<CodeFoldRegion>();
        AddPythonRegions(lines, regions);
        AddBraceRegions(lines, regions);
        return regions
            .Where(region => region.EndIndex > region.StartIndex)
            .GroupBy(region => region.StartIndex)
            .Select(group => group.OrderByDescending(region => region.EndIndex).First())
            .OrderBy(region => region.StartIndex)
            .ToArray();
    }

    private static void AddPythonRegions(
        IReadOnlyList<DiffLine> lines,
        ICollection<CodeFoldRegion> regions)
    {
        for (int index = 0; index < lines.Count; index++)
        {
            Match match = PythonFunctionRegex.Match(lines[index].Text);
            if (!match.Success)
            {
                continue;
            }

            int indentation = match.Groups["indent"].Value.Length;
            int end = index;
            for (int candidate = index + 1; candidate < lines.Count; candidate++)
            {
                string text = lines[candidate].Text;
                if (string.IsNullOrWhiteSpace(text))
                {
                    continue;
                }

                int candidateIndentation = text.Length - text.TrimStart().Length;
                if (candidateIndentation <= indentation)
                {
                    break;
                }

                end = candidate;
            }

            regions.Add(new CodeFoldRegion(
                index,
                end,
                match.Groups["name"].Value));
        }
    }

    private static void AddBraceRegions(
        IReadOnlyList<DiffLine> lines,
        ICollection<CodeFoldRegion> regions)
    {
        for (int index = 0; index < lines.Count; index++)
        {
            string current = StripStringsAndComments(lines[index].Text);
            int openingBrace = current.IndexOf('{');
            if (openingBrace < 0)
            {
                continue;
            }

            int start = Math.Max(0, index - 6);
            string header = string.Join(
                " ",
                lines.Skip(start)
                    .Take(index - start + 1)
                    .Select(line => StripStringsAndComments(line.Text).Trim()))
                .Replace('\t', ' ');
            if (header.Contains(';') &&
                header.IndexOf(';', StringComparison.Ordinal) >
                header.IndexOf('(', StringComparison.Ordinal))
            {
                header = header[(header.LastIndexOf(';') + 1)..];
            }

            Match match = FunctionRegex.Match(header);
            if (!match.Success ||
                ControlNames.Contains(match.Groups["name"].Value) ||
                IsTypeDeclaration(header))
            {
                continue;
            }

            int functionStart = FindFunctionStart(lines, start, index);
            int depth = CountBraces(current);
            int end = index;
            while (depth > 0 && end + 1 < lines.Count)
            {
                end++;
                depth += CountBraces(StripStringsAndComments(lines[end].Text));
            }

            if (depth == 0)
            {
                regions.Add(new CodeFoldRegion(
                    functionStart,
                    end,
                    match.Groups["name"].Value));
            }
        }
    }

    private static int FindFunctionStart(
        IReadOnlyList<DiffLine> lines,
        int lowerBound,
        int braceIndex)
    {
        int start = braceIndex;
        for (int index = braceIndex - 1; index >= lowerBound; index--)
        {
            string text = StripStringsAndComments(lines[index].Text).Trim();
            if (string.IsNullOrWhiteSpace(text) ||
                text.EndsWith(';') ||
                text.EndsWith('{') ||
                text.EndsWith('}'))
            {
                break;
            }

            start = index;
            if (text.Contains('('))
            {
                break;
            }
        }

        return start;
    }

    private static bool IsTypeDeclaration(string header) =>
        Regex.IsMatch(
            header,
            @"\b(class|struct|enum|union|namespace|interface)\s+[A-Za-z_]\w*\s*(?:[:{])",
            RegexOptions.CultureInvariant);

    private static int CountBraces(string line) =>
        line.Count(character => character == '{') -
        line.Count(character => character == '}');

    private static string StripStringsAndComments(string value)
    {
        var output = new char[value.Length];
        bool inString = false;
        bool inCharacter = false;
        bool escape = false;
        for (int index = 0; index < value.Length; index++)
        {
            char current = value[index];
            if (escape)
            {
                output[index] = ' ';
                escape = false;
                continue;
            }

            if (!inCharacter && !inString &&
                current == '/' &&
                index + 1 < value.Length &&
                value[index + 1] == '/')
            {
                Array.Fill(output, ' ', index, output.Length - index);
                break;
            }

            if (current == '\\' && (inString || inCharacter))
            {
                output[index] = ' ';
                escape = true;
                continue;
            }

            if (current == '"' && !inCharacter)
            {
                inString = !inString;
                output[index] = ' ';
                continue;
            }

            if (current == '\'' && !inString)
            {
                inCharacter = !inCharacter;
                output[index] = ' ';
                continue;
            }

            output[index] = inString || inCharacter ? ' ' : current;
        }

        return new string(output);
    }
}
