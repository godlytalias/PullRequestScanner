using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Providers;

public interface IReviewProviderClient
{
    ReviewProvider Provider { get; }

    Task<PullRequestDetails> GetPullRequestAsync(
        PullRequestReference reference,
        CancellationToken cancellationToken = default);

    Task<IReadOnlyList<PullRequestIteration>> GetIterationsAsync(
        PullRequestDetails pullRequest,
        CancellationToken cancellationToken = default);

    Task<IReadOnlyList<RemoteReviewComment>> GetCommentsAsync(
        PullRequestDetails pullRequest,
        CancellationToken cancellationToken = default);

    Task<string> PublishCommentAsync(
        PullRequestDetails pullRequest,
        ReviewComment comment,
        CancellationToken cancellationToken = default);

    Task DeleteCommentAsync(
        PullRequestDetails pullRequest,
        ReviewComment comment,
        CancellationToken cancellationToken = default);

    Task<string> ReplyToCommentAsync(
        PullRequestDetails pullRequest,
        RemoteReviewComment parentComment,
        string body,
        CancellationToken cancellationToken = default);
}
