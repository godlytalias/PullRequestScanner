namespace PullRequestScanner.Core.Models;

public static class ReviewActionState
{
    public static bool CanSaveComment(
        bool hasSession,
        bool hasCommentableLine,
        string comment,
        string suggestion,
        bool isBusy) =>
        hasSession &&
        hasCommentableLine &&
        !isBusy &&
        (!string.IsNullOrWhiteSpace(comment) ||
         !string.IsNullOrWhiteSpace(suggestion));

    public static bool CanPublishComments(
        int pendingCount,
        bool isBusy) =>
        pendingCount > 0 &&
        !isBusy;
}
