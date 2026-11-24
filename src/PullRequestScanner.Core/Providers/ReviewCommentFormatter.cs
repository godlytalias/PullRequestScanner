using System.Text;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Providers;

public static class ReviewCommentFormatter
{
    public static string CreatePublishedBody(
        ReviewProvider provider,
        ReviewComment comment)
    {
        var builder = new StringBuilder(comment.Body.Trim());
        if (!string.IsNullOrWhiteSpace(comment.SelectedText))
        {
            AppendSeparator(builder);
            builder.Append("Selected code:\n");
            foreach (string selectedLine in comment.SelectedText
                         .Replace("\r\n", "\n", StringComparison.Ordinal)
                         .Split('\n'))
            {
                builder.Append("> ");
                builder.Append(selectedLine);
                builder.Append('\n');
            }
        }

        if (string.IsNullOrWhiteSpace(comment.SuggestedReplacement))
        {
            return builder.ToString().TrimEnd();
        }

        string replacementLine = BuildSuggestedLine(comment);
        AppendSeparator(builder);
        if (provider == ReviewProvider.GitHub &&
            comment.Side == ReviewCommentSide.Right)
        {
            builder.Append("```suggestion\n");
            builder.Append(replacementLine);
            builder.Append('\n');
            builder.Append("```");
        }
        else
        {
            builder.Append("Suggested replacement:\n```diff\n");
            if (!string.IsNullOrEmpty(comment.SourceLineText))
            {
                builder.Append("- ");
                builder.Append(comment.SourceLineText);
                builder.Append('\n');
            }

            builder.Append("+ ");
            builder.Append(replacementLine);
            builder.Append('\n');
            builder.Append("```");
        }

        return builder.ToString().TrimEnd();
    }

    public static string BuildSuggestedLine(ReviewComment comment)
    {
        if (comment.SelectionStart is not int selectionStart ||
            selectionStart < 0 ||
            selectionStart > comment.SourceLineText.Length ||
            comment.SelectionLength <= 0)
        {
            return comment.SuggestedReplacement;
        }

        int selectionLength = Math.Min(
            comment.SelectionLength,
            comment.SourceLineText.Length - selectionStart);
        return string.Concat(
            comment.SourceLineText.AsSpan(0, selectionStart),
            comment.SuggestedReplacement,
            comment.SourceLineText.AsSpan(selectionStart + selectionLength));
    }

    private static void AppendSeparator(StringBuilder builder)
    {
        if (builder.Length > 0 && !builder.ToString().EndsWith("\n\n", StringComparison.Ordinal))
        {
            builder.Append("\n\n");
        }
    }
}
