using System.Text.RegularExpressions;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Diff;

public static partial class UnifiedDiffParser
{
    public static IReadOnlyList<DiffBlock> Parse(string diff)
    {
        var blocks = new List<DiffBlock>();
        string filePath = string.Empty;
        string? previousPath = null;
        List<DiffLine>? lines = null;
        int oldLine = 0;
        int newLine = 0;

        void FinishBlock()
        {
            if (lines is null)
            {
                return;
            }

            blocks.Add(new DiffBlock
            {
                FilePath = filePath,
                PreviousPath = previousPath,
                Lines = lines.ToArray()
            });
            lines = null;
        }

        string normalized = diff.Replace("\r\n", "\n", StringComparison.Ordinal);
        foreach (string line in normalized.Split('\n'))
        {
            if (line.StartsWith("diff --git ", StringComparison.Ordinal))
            {
                FinishBlock();
                var paths = ParseDiffHeader(line);
                previousPath = paths.oldPath;
                filePath = paths.newPath;
                lines = [];
                oldLine = 0;
                newLine = 0;
                continue;
            }

            if (lines is null)
            {
                continue;
            }

            if (line.StartsWith("rename from ", StringComparison.Ordinal))
            {
                previousPath = line[12..].Trim();
                continue;
            }

            if (line.StartsWith("rename to ", StringComparison.Ordinal))
            {
                filePath = line[10..].Trim();
                continue;
            }

            if (line.StartsWith("--- ", StringComparison.Ordinal) ||
                line.StartsWith("+++ ", StringComparison.Ordinal))
            {
                if (line.StartsWith("+++ ", StringComparison.Ordinal) &&
                    line.Length > 4 &&
                    line != "+++ /dev/null")
                {
                    filePath = StripPathPrefix(line[4..]);
                }
                continue;
            }

            Match hunkMatch = HunkHeaderRegex().Match(line);
            if (hunkMatch.Success)
            {
                oldLine = int.Parse(hunkMatch.Groups["old"].Value);
                newLine = int.Parse(hunkMatch.Groups["new"].Value);
                lines.Add(new DiffLine
                {
                    Kind = DiffLineKind.Hunk,
                    FilePath = filePath,
                    Text = line
                });
                continue;
            }

            if (line.StartsWith(@"\ No newline at end of file", StringComparison.Ordinal))
            {
                continue;
            }

            if (oldLine == 0 && newLine == 0)
            {
                if (line.StartsWith("Binary files ", StringComparison.Ordinal) ||
                    line.StartsWith("GIT binary patch", StringComparison.Ordinal))
                {
                    lines.Add(new DiffLine
                    {
                        Kind = DiffLineKind.Hunk,
                        FilePath = filePath,
                        Text = line
                    });
                }

                continue;
            }

            if (line.StartsWith('+'))
            {
                lines.Add(new DiffLine
                {
                    Kind = DiffLineKind.Added,
                    FilePath = filePath,
                    NewLineNumber = newLine++,
                    Text = line[1..]
                });
            }
            else if (line.StartsWith('-'))
            {
                lines.Add(new DiffLine
                {
                    Kind = DiffLineKind.Deleted,
                    FilePath = filePath,
                    OldLineNumber = oldLine++,
                    Text = line[1..]
                });
            }
            else if (line.StartsWith(' '))
            {
                lines.Add(new DiffLine
                {
                    Kind = DiffLineKind.Context,
                    FilePath = filePath,
                    OldLineNumber = oldLine++,
                    NewLineNumber = newLine++,
                    Text = line[1..]
                });
            }
        }

        FinishBlock();
        return blocks;
    }

    private static (string oldPath, string newPath) ParseDiffHeader(string line)
    {
        string value = line["diff --git ".Length..];
        Match match = DiffHeaderRegex().Match(value);
        if (!match.Success)
        {
            return (StripPathPrefix(value), StripPathPrefix(value));
        }

        return (
            StripPathPrefix(Unquote(match.Groups["old"].Value)),
            StripPathPrefix(Unquote(match.Groups["new"].Value)));
    }

    private static string StripPathPrefix(string path)
    {
        string value = path.Trim();
        if (value.StartsWith("a/", StringComparison.Ordinal) ||
            value.StartsWith("b/", StringComparison.Ordinal))
        {
            value = value[2..];
        }

        return Unquote(value);
    }

    private static string Unquote(string value)
    {
        if (value.Length >= 2 && value[0] == '"' && value[^1] == '"')
        {
            return value[1..^1]
                .Replace("\\\"", "\"", StringComparison.Ordinal)
                .Replace("\\\\", "\\", StringComparison.Ordinal);
        }

        return value;
    }

    [GeneratedRegex("""^@@ -(?<old>\d+)(?:,\d+)? \+(?<new>\d+)(?:,\d+)? @@.*$""", RegexOptions.CultureInvariant)]
    private static partial Regex HunkHeaderRegex();

    [GeneratedRegex("""^(?<old>"(?:[^"\\]|\\.)*"|\S+)\s+(?<new>"(?:[^"\\]|\\.)*"|\S+)$""", RegexOptions.CultureInvariant)]
    private static partial Regex DiffHeaderRegex();
}
