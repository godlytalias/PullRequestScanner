using PullRequestScanner.Core.Authentication;
using PullRequestScanner.Core.Diff;
using PullRequestScanner.Core.Exceptions;
using PullRequestScanner.Core.Models;
using PullRequestScanner.Core.Navigation;
using PullRequestScanner.Core.Persistence;
using PullRequestScanner.Core.Providers;
using PullRequestScanner.Core.Repository;

namespace PullRequestScanner.Core.Services;

public sealed record CommentPublishResult(int Published, int Failed);
public sealed record PullRequestReviewRange(
    string BaseCommitSha,
    string HeadCommitSha,
    string? IterationId);

public sealed class ReviewSessionService(
    ReviewProviderFactory providerFactory,
    LocalRepositoryService repositoryService,
    GitDiffService diffService,
    SymbolNavigationService symbolNavigationService,
    JsonReviewStateStore stateStore,
    ICredentialProvider credentialProvider,
    ReviewActivityService activityService)
{
    public JsonReviewStateStore StateStore => stateStore;

    public async Task<ReviewSessionState> OpenAsync(
        string pullRequestUrl,
        string? localRepositoryPath = null,
        CancellationToken cancellationToken = default)
    {
        PullRequestReference reference = PullRequestUrlParser.Parse(pullRequestUrl);
        IReviewProviderClient provider = providerFactory.Get(reference.Provider);
        PullRequestDetails details = await provider.GetPullRequestAsync(reference, cancellationToken);
        ReviewSessionState? existing = await stateStore.LoadAsync(reference.SessionKey, cancellationToken);
        ReviewSessionState state = existing ?? new ReviewSessionState
        {
            Details = details
        };
        state.Details = details;

        string? repositoryPath = string.IsNullOrWhiteSpace(localRepositoryPath)
            ? state.LocalRepositoryPath
            : localRepositoryPath;
        LocalRepository repository = await repositoryService.PrepareAsync(
            details,
            repositoryPath,
            cancellationToken);
        state.LocalRepositoryPath = repository.Path;
        state.ReusedExistingRepository = repository.ReusedExistingCheckout;
        state.Iterations = (await provider.GetIterationsAsync(details, cancellationToken)).ToList();
        NormalizeSelectedIteration(state);
        PullRequestReviewRange range = GetActiveRange(state);
        state.ChangedFiles = (await diffService.ListChangedFilesAsync(
            state,
            range.BaseCommitSha,
            range.HeadCommitSha,
            cancellationToken)).ToList();
        state.RemoteComments = (await provider.GetCommentsAsync(details, cancellationToken)).ToList();
        state.LastOpenedUtc = DateTimeOffset.UtcNow;
        await stateStore.SaveAsync(state, cancellationToken);
        await activityService.RecordOpenedAsync(state, cancellationToken);
        return state;
    }

    public async Task<ReviewSessionState> RefreshRepositoryAsync(
        ReviewSessionState state,
        CancellationToken cancellationToken = default)
    {
        IReviewProviderClient provider = providerFactory.Get(state.Details.Reference.Provider);
        PullRequestDetails details = await provider.GetPullRequestAsync(
            state.Details.Reference,
            cancellationToken);
        state.Details = details;
        LocalRepository repository = await repositoryService.PrepareAsync(
            details,
            !state.ReusedExistingRepository &&
            repositoryService.IsManagedCachePath(state.LocalRepositoryPath)
                ? null
                : state.LocalRepositoryPath,
            cancellationToken);
        state.LocalRepositoryPath = repository.Path;
        state.ReusedExistingRepository = repository.ReusedExistingCheckout;
        state.Iterations = (await provider.GetIterationsAsync(details, cancellationToken)).ToList();
        NormalizeSelectedIteration(state);
        PullRequestReviewRange range = GetActiveRange(state);
        state.ChangedFiles = (await diffService.ListChangedFilesAsync(
            state,
            range.BaseCommitSha,
            range.HeadCommitSha,
            cancellationToken)).ToList();
        state.RemoteComments = (await provider.GetCommentsAsync(details, cancellationToken)).ToList();
        await stateStore.SaveAsync(state, cancellationToken);
        return state;
    }

    public async Task<ReviewSessionState> RefreshSummaryAsync(
        ReviewSessionState state,
        CancellationToken cancellationToken = default)
    {
        IReviewProviderClient provider = providerFactory.Get(state.Details.Reference.Provider);
        PullRequestDetails details = await provider.GetPullRequestAsync(
            state.Details.Reference,
            cancellationToken);
        IReadOnlyList<PullRequestIteration> iterations = await provider.GetIterationsAsync(
            details,
            cancellationToken);
        state.Details = details;
        state.Iterations = iterations.ToList();
        NormalizeSelectedIteration(state);
        await stateStore.SaveAsync(state, cancellationToken);
        return state;
    }

    public async Task<ReviewSessionState> SelectIterationAsync(
        ReviewSessionState state,
        string? iterationId,
        CancellationToken cancellationToken = default)
    {
        state.SelectedIterationId = iterationId;
        NormalizeSelectedIteration(state);
        PullRequestReviewRange range = GetActiveRange(state);
        state.ChangedFiles = (await diffService.ListChangedFilesAsync(
            state,
            range.BaseCommitSha,
            range.HeadCommitSha,
            cancellationToken)).ToList();
        await stateStore.SaveAsync(state, cancellationToken);
        return state;
    }

    public async Task MarkIterationReviewedAsync(
        ReviewSessionState state,
        CancellationToken cancellationToken = default)
    {
        PullRequestIteration latest = state.Iterations.LastOrDefault() ??
            throw new ReviewOperationException("This pull request has no review iterations yet.");
        state.LastReviewedIterationId = latest.Id;
        state.LastReviewedAtUtc = DateTimeOffset.UtcNow;
        await stateStore.SaveAsync(state, cancellationToken);
        await activityService.RecordReviewedAsync(state, cancellationToken);
    }

    public static PullRequestReviewRange GetActiveRange(ReviewSessionState state)
    {
        PullRequestIteration? selected = state.Iterations.FirstOrDefault(
            iteration => iteration.Id.Equals(
                state.SelectedIterationId,
                StringComparison.Ordinal));
        return selected is null
            ? new PullRequestReviewRange(
                state.Details.BaseCommitSha,
                state.Details.HeadCommitSha,
                null)
            : new PullRequestReviewRange(
                selected.BaseCommitSha,
                selected.HeadCommitSha,
                selected.Id);
    }

    public Task<IReadOnlyList<DiffBlock>> LoadDiffAsync(
        ReviewSessionState state,
        string pathSpec,
        int contextLines,
        bool completeFile,
        CancellationToken cancellationToken = default) =>
        diffService.GetDiffAsync(
            state,
            pathSpec,
            contextLines,
            completeFile,
            GetActiveRange(state).BaseCommitSha,
            GetActiveRange(state).HeadCommitSha,
            cancellationToken);

    public Task<string> ReadFileAsync(
        ReviewSessionState state,
        string path,
        string? commit = null,
        CancellationToken cancellationToken = default) =>
        diffService.ReadFileAsync(
            state,
            commit ?? state.Details.HeadCommitSha,
            path,
            cancellationToken);

    public Task<SymbolLocation?> FindDefinitionAsync(
        ReviewSessionState state,
        string symbol,
        string preferredPath,
        string? commit = null,
        int? sourceLineNumber = null,
        CancellationToken cancellationToken = default) =>
        symbolNavigationService.FindDefinitionAsync(
            state,
            symbol,
            preferredPath,
            commit,
            sourceLineNumber,
            cancellationToken);

    public async Task<CommentPublishResult> PublishPendingCommentsAsync(
        ReviewSessionState state,
        CancellationToken cancellationToken = default)
    {
        IReviewProviderClient provider = providerFactory.Get(state.Details.Reference.Provider);
        int published = 0;
        int failed = 0;

        foreach (ReviewComment comment in state.Comments.Where(
                     comment => comment.Status is ReviewCommentStatus.Pending or ReviewCommentStatus.Failed))
        {
            try
            {
                string remoteId = await provider.PublishCommentAsync(
                    state.Details,
                    comment,
                    cancellationToken);
                comment.RemoteId = remoteId;
                comment.Error = null;
                comment.Status = ReviewCommentStatus.Published;
                comment.PublishedAtUtc = DateTimeOffset.UtcNow;
                published++;
                await stateStore.SaveAsync(state, cancellationToken);
                await activityService.RecordPublishedCommentAsync(
                    state,
                    remoteId,
                    cancellationToken);
            }
            catch (ReviewAuthenticationException exception)
            {
                comment.Status = ReviewCommentStatus.Failed;
                comment.Error = exception.Message;
                failed++;
                await stateStore.SaveAsync(state, cancellationToken);
                throw;
            }
            catch (ReviewOperationException exception)
            {
                comment.Status = ReviewCommentStatus.Failed;
                comment.Error = exception.Message;
                failed++;
                await stateStore.SaveAsync(state, cancellationToken);
            }
            catch (HttpRequestException exception)
            {
                comment.Status = ReviewCommentStatus.Failed;
                comment.Error = exception.Message;
                failed++;
                await stateStore.SaveAsync(state, cancellationToken);
            }
        }

        return new CommentPublishResult(published, failed);
    }

    public async Task DeleteCommentAsync(
        ReviewSessionState state,
        ReviewComment comment,
        CancellationToken cancellationToken = default)
    {
        bool wasPublished = comment.Status == ReviewCommentStatus.Published &&
            !string.IsNullOrWhiteSpace(comment.RemoteId);
        if (wasPublished &&
            !string.IsNullOrWhiteSpace(comment.RemoteId))
        {
            IReviewProviderClient provider = providerFactory.Get(
                state.Details.Reference.Provider);
            await provider.DeleteCommentAsync(
                state.Details,
                comment,
                cancellationToken);
        }

        if (!state.Comments.Remove(comment))
        {
            throw new ReviewOperationException("The comment is no longer part of this saved review.");
        }

        if (wasPublished)
        {
            state.RemoteComments.RemoveAll(existing =>
                existing.RemoteId.Equals(
                    comment.RemoteId,
                    StringComparison.Ordinal));
        }

        await stateStore.SaveAsync(state, cancellationToken);
    }

    public async Task<string> ReplyToCommentAsync(
        ReviewSessionState state,
        RemoteReviewComment parentComment,
        string body,
        CancellationToken cancellationToken = default)
    {
        IReviewProviderClient provider = providerFactory.Get(
            state.Details.Reference.Provider);
        string remoteId = await provider.ReplyToCommentAsync(
            state.Details,
            parentComment,
            body,
            cancellationToken);
        state.RemoteComments = (await provider.GetCommentsAsync(
            state.Details,
            cancellationToken)).ToList();
        await stateStore.SaveAsync(state, cancellationToken);
        await activityService.RecordReplyAsync(
            state,
            remoteId,
            cancellationToken);
        return remoteId;
    }

    public async Task CompleteReviewAsync(
        ReviewSessionState state,
        CancellationToken cancellationToken = default)
    {
        state.CompletedAtUtc = DateTimeOffset.UtcNow;
        await activityService.RecordCompletedAsync(state, cancellationToken);

        IReadOnlyList<ReviewSessionState> remaining =
            await stateStore.LoadAllAsync(cancellationToken);
        bool cacheIsShared = remaining.Any(other =>
            !other.Details.Reference.SessionKey.Equals(
                state.Details.Reference.SessionKey,
                StringComparison.Ordinal) &&
            string.Equals(
                other.LocalRepositoryPath,
                state.LocalRepositoryPath,
                StringComparison.OrdinalIgnoreCase));

        if (!state.ReusedExistingRepository && !cacheIsShared)
        {
            await repositoryService.DeleteCachedRepositoryAsync(
                state.LocalRepositoryPath,
                cancellationToken);
        }

        await stateStore.DeleteAsync(state, cancellationToken);
    }

    public Task RecordOpenedAsync(
        ReviewSessionState state,
        CancellationToken cancellationToken = default) =>
        activityService.RecordOpenedAsync(state, cancellationToken);

    public Task SaveAsync(
        ReviewSessionState state,
        CancellationToken cancellationToken = default) =>
        stateStore.SaveAsync(state, cancellationToken);

    public Task DeleteAsync(
        ReviewSessionState state,
        CancellationToken cancellationToken = default) =>
        stateStore.DeleteAsync(state, cancellationToken);

    public Task AuthenticateAsync(
        PullRequestReference reference,
        CancellationToken cancellationToken = default) =>
        credentialProvider.AuthenticateAsync(reference, cancellationToken);

    private static void NormalizeSelectedIteration(ReviewSessionState state)
    {
        if (!string.IsNullOrWhiteSpace(state.SelectedIterationId) &&
            !state.Iterations.Any(iteration =>
                iteration.Id.Equals(state.SelectedIterationId, StringComparison.Ordinal)))
        {
            state.SelectedIterationId = null;
        }
    }
}
