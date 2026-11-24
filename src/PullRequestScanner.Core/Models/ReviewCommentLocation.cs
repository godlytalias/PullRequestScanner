namespace PullRequestScanner.Core.Models;

public static class ReviewCommentLocation
{
    public const string Conversation = "Pull request conversation";

    public static string Format(string? filePath, int? lineNumber)
    {
        if (string.IsNullOrWhiteSpace(filePath) || lineNumber is null)
        {
            return Conversation;
        }

        return $"{filePath}:{lineNumber}";
    }
}
