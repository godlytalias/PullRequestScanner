using System.Text;
using System.Text.RegularExpressions;
using System.Net;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Markdown;

public static class MarkdownParser
{
    public static IReadOnlyList<MarkdownBlock> Parse(string markdown)
    {
        string[] lines = (markdown ?? string.Empty)
            .Replace("\r\n", "\n", StringComparison.Ordinal)
            .Replace('\r', '\n')
            .Split('\n');
        var blocks = new List<MarkdownBlock>();
        for (int index = 0; index < lines.Length;)
        {
            string line = lines[index];
            if (string.IsNullOrWhiteSpace(line))
            {
                index++;
                continue;
            }

            string trimmed = line.TrimStart();
            if (trimmed.StartsWith("```", StringComparison.Ordinal))
            {
                string language = trimmed[3..].Trim();
                int start = index + 1;
                var code = new StringBuilder();
                index++;
                int count = 0;
                while (index < lines.Length &&
                       !lines[index].TrimStart().StartsWith("```", StringComparison.Ordinal))
                {
                    if (code.Length > 0)
                    {
                        code.AppendLine();
                    }

                    code.Append(lines[index]);
                    index++;
                    count++;
                }

                blocks.Add(new MarkdownBlock(
                    MarkdownBlockKind.Code,
                    code.ToString(),
                    start + 1,
                    Math.Max(1, count),
                    language));
                if (index < lines.Length)
                {
                    index++;
                }

                continue;
            }

            int heading = CountPrefix(trimmed, '#');
            if (heading is >= 1 and <= 6 &&
                trimmed.Length > heading &&
                trimmed[heading] == ' ')
            {
                string headingText = trimmed[(heading + 1)..];
                blocks.Add(new MarkdownBlock(
                    heading switch
                    {
                        1 => MarkdownBlockKind.Heading1,
                        2 => MarkdownBlockKind.Heading2,
                        3 => MarkdownBlockKind.Heading3,
                        4 => MarkdownBlockKind.Heading4,
                        5 => MarkdownBlockKind.Heading5,
                        _ => MarkdownBlockKind.Heading6
                    },
                    FormatInline(headingText),
                    index + 1,
                    InlineRuns: ParseInlineRuns(headingText)));
                index++;
                continue;
            }

            if (trimmed is "---" or "***" or "___")
            {
                blocks.Add(new MarkdownBlock(
                    MarkdownBlockKind.HorizontalRule,
                    string.Empty,
                    index + 1));
                index++;
                continue;
            }

            Match task = Regex.Match(
                trimmed,
                @"^[-+*]\s+\[([ xX])\]\s+(.+)$");
            if (task.Success)
            {
                bool isChecked = task.Groups[1].Value.Equals(
                    "x",
                    StringComparison.OrdinalIgnoreCase);
                blocks.Add(new MarkdownBlock(
                    isChecked
                        ? MarkdownBlockKind.TaskChecked
                        : MarkdownBlockKind.TaskUnchecked,
                    FormatInline(task.Groups[2].Value),
                    index + 1,
                    ListMarker: isChecked ? "\uE73E" : "\uE739",
                    InlineRuns: ParseInlineRuns(task.Groups[2].Value)));
                index++;
                continue;
            }

            if (Regex.IsMatch(trimmed, @"^[-+*]\s+"))
            {
                blocks.Add(new MarkdownBlock(
                    MarkdownBlockKind.Bullet,
                    FormatInline(trimmed[2..].Trim()),
                    index + 1,
                    ListMarker: "\u2022",
                    InlineRuns: ParseInlineRuns(trimmed[2..].Trim())));
                index++;
                continue;
            }

            if (trimmed.Contains('|', StringComparison.Ordinal) &&
                index + 1 < lines.Length &&
                Regex.IsMatch(
                    lines[index + 1].Trim(),
                    @"^\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)+\|?$"))
            {
                int tableStartIndex = index;
                var table = new StringBuilder();
                while (index < lines.Length &&
                       lines[index].Trim().Contains('|', StringComparison.Ordinal))
                {
                    if (table.Length > 0)
                    {
                        table.AppendLine();
                    }

                    table.Append(lines[index].Trim());
                    index++;
                }

                blocks.Add(new MarkdownBlock(
                    MarkdownBlockKind.Table,
                    table.ToString(),
                    tableStartIndex + 1,
                    index - tableStartIndex));
                continue;
            }

            Match numbered = Regex.Match(trimmed, @"^\d+[.)]\s+");
            if (numbered.Success)
            {
                blocks.Add(new MarkdownBlock(
                    MarkdownBlockKind.Numbered,
                    FormatInline(trimmed[numbered.Length..]),
                    index + 1,
                    ListMarker: numbered.Value.Trim(),
                    InlineRuns: ParseInlineRuns(trimmed[numbered.Length..])));
                index++;
                continue;
            }

            if (trimmed.StartsWith('>'))
            {
                blocks.Add(new MarkdownBlock(
                    MarkdownBlockKind.Quote,
                    FormatInline(trimmed[1..].TrimStart()),
                    index + 1,
                    InlineRuns: ParseInlineRuns(trimmed[1..].TrimStart())));
                index++;
                continue;
            }

            int startIndex = index;
            var paragraph = new StringBuilder();
            do
            {
                if (paragraph.Length > 0)
                {
                    paragraph.Append(' ');
                }

                paragraph.Append(lines[index].Trim());
                index++;
            }
            while (index < lines.Length &&
                   !string.IsNullOrWhiteSpace(lines[index]) &&
                   IsParagraphLine(lines[index]));

            blocks.Add(new MarkdownBlock(
                MarkdownBlockKind.Paragraph,
                FormatInline(paragraph.ToString()),
                startIndex + 1,
                index - startIndex,
                InlineRuns: ParseInlineRuns(paragraph.ToString())));
        }

        return blocks;
    }

    public static string ToPlainText(string markdown)
    {
        string value = (markdown ?? string.Empty)
            .Replace("<br>", "\n", StringComparison.OrdinalIgnoreCase)
            .Replace("<br/>", "\n", StringComparison.OrdinalIgnoreCase)
            .Replace("<br />", "\n", StringComparison.OrdinalIgnoreCase);
        value = Regex.Replace(
            value,
            "<!--.*?-->",
            string.Empty,
            RegexOptions.Singleline);
        value = Regex.Replace(
            value,
            "<[^>]+>",
            string.Empty,
            RegexOptions.Singleline);
        value = WebUtility.HtmlDecode(value);
        return FormatInline(value);
    }

    private static bool IsParagraphLine(string line)
    {
        string trimmed = line.TrimStart();
        return !trimmed.StartsWith("```", StringComparison.Ordinal) &&
               CountPrefix(trimmed, '#') == 0 &&
               !Regex.IsMatch(trimmed, @"^[-+*]\s+") &&
               !Regex.IsMatch(trimmed, @"^\d+[.)]\s+") &&
               !trimmed.StartsWith('>') &&
               trimmed is not "---" and not "***" and not "___";
    }

    private static int CountPrefix(string value, char character)
    {
        int count = 0;
        while (count < value.Length && value[count] == character)
        {
            count++;
        }

        return count;
    }

    private static string FormatInline(string value)
    {
        string result = Regex.Replace(
            value,
            @"!\[([^\]]*)\]\(([^)]+)\)",
            "$1");
        result = Regex.Replace(
            result,
            @"\[([^\]]+)\]\(([^)]+)\)",
            "$1 ($2)");
        result = result
            .Replace("**", string.Empty, StringComparison.Ordinal)
            .Replace("__", string.Empty, StringComparison.Ordinal)
            .Replace("`", string.Empty, StringComparison.Ordinal);
        return result.Trim();
    }

    private static IReadOnlyList<MarkdownInlineRun> ParseInlineRuns(string value)
    {
        var runs = new List<MarkdownInlineRun>();
        var pattern = new Regex(
            @"(?<bold>\*\*(?<boldText>.+?)\*\*)|" +
            @"(?<code>`(?<codeText>[^`]+?)`)|" +
            @"(?<link>\[(?<linkText>[^\]]+)\]\((?<linkUrl>[^)]+)\))",
            RegexOptions.CultureInvariant);
        int position = 0;
        foreach (Match match in pattern.Matches(value))
        {
            if (match.Index > position)
            {
                runs.Add(new MarkdownInlineRun(
                    MarkdownInlineKind.Text,
                    WebUtility.HtmlDecode(value[position..match.Index])));
            }

            if (match.Groups["bold"].Success)
            {
                runs.Add(new MarkdownInlineRun(
                    MarkdownInlineKind.Bold,
                    WebUtility.HtmlDecode(match.Groups["boldText"].Value)));
            }
            else if (match.Groups["code"].Success)
            {
                runs.Add(new MarkdownInlineRun(
                    MarkdownInlineKind.Code,
                    match.Groups["codeText"].Value));
            }
            else if (match.Groups["link"].Success &&
                     Uri.TryCreate(
                         match.Groups["linkUrl"].Value,
                         UriKind.Absolute,
                         out Uri? uri))
            {
                runs.Add(new MarkdownInlineRun(
                    MarkdownInlineKind.Link,
                    WebUtility.HtmlDecode(match.Groups["linkText"].Value),
                    uri));
            }
            else
            {
                runs.Add(new MarkdownInlineRun(
                    MarkdownInlineKind.Text,
                    WebUtility.HtmlDecode(match.Value)));
            }

            position = match.Index + match.Length;
        }

        if (position < value.Length)
        {
            runs.Add(new MarkdownInlineRun(
                MarkdownInlineKind.Text,
                WebUtility.HtmlDecode(value[position..])));
        }

        return runs.Count == 0
            ? [new MarkdownInlineRun(MarkdownInlineKind.Text, string.Empty)]
            : runs;
    }
}
