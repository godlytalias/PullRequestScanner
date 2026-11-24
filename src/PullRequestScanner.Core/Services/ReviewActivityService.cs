using PullRequestScanner.Core.Models;
using PullRequestScanner.Core.Persistence;

namespace PullRequestScanner.Core.Services;

public sealed record ReviewActivityDay(
    DateOnly Date,
    int PullRequestCount,
    int CommentCount,
    int ReviewCount);

public sealed class ReviewActivityService(JsonReviewActivityStore store)
{
    public Task RecordOpenedAsync(
        ReviewSessionState state,
        CancellationToken cancellationToken = default) =>
        RecordAsync(
            state,
            ReviewActivityKind.Opened,
            $"opened:{state.Details.Reference.SessionKey}:{DateTimeOffset.UtcNow:yyyy-MM-dd}",
            cancellationToken);

    public Task RecordPublishedCommentAsync(
        ReviewSessionState state,
        string remoteId,
        CancellationToken cancellationToken = default) =>
        RecordAsync(
            state,
            ReviewActivityKind.CommentPublished,
            $"comment:{remoteId}",
            cancellationToken,
            commentDelta: 1);

    public Task RecordReplyAsync(
        ReviewSessionState state,
        string remoteId,
        CancellationToken cancellationToken = default) =>
        RecordAsync(
            state,
            ReviewActivityKind.ReplyPublished,
            $"reply:{remoteId}",
            cancellationToken,
            commentDelta: 1);

    public Task RecordReviewedAsync(
        ReviewSessionState state,
        CancellationToken cancellationToken = default) =>
        RecordAsync(
            state,
            ReviewActivityKind.MarkedReviewed,
            $"reviewed:{state.Details.Reference.SessionKey}:{state.LastReviewedIterationId}",
            cancellationToken);

    public Task RecordCompletedAsync(
        ReviewSessionState state,
        CancellationToken cancellationToken = default) =>
        RecordAsync(
            state,
            ReviewActivityKind.Completed,
            $"completed:{state.Details.Reference.SessionKey}",
            cancellationToken);

    public async Task<IReadOnlyList<ReviewActivityDay>> GetMonthAsync(
        int year,
        int month,
        CancellationToken cancellationToken = default)
    {
        IReadOnlyList<ReviewActivityRecord> records = await store.LoadAsync(
            cancellationToken);
        DateOnly start = new(year, month, 1);
        DateOnly end = start.AddMonths(1);
        ReviewActivityRecord[] monthRecords = records
            .Where(record =>
            {
                DateOnly date = DateOnly.FromDateTime(
                    record.AtUtc.ToLocalTime().DateTime);
                return date >= start && date < end;
            })
            .ToArray();

        return Enumerable.Range(0, end.DayNumber - start.DayNumber)
            .Select(offset =>
            {
                DateOnly date = start.AddDays(offset);
                ReviewActivityRecord[] day = monthRecords
                    .Where(record =>
                        DateOnly.FromDateTime(
                            record.AtUtc.ToLocalTime().DateTime) == date)
                    .ToArray();
                return new ReviewActivityDay(
                    date,
                    day
                        .Where(record => record.Kind is
                            ReviewActivityKind.Opened or
                            ReviewActivityKind.Completed or
                            ReviewActivityKind.MarkedReviewed)
                        .Select(record => record.SessionKey)
                        .Distinct(StringComparer.Ordinal)
                        .Count(),
                    day.Sum(record => record.CommentDelta),
                    day.Count(record => record.Kind == ReviewActivityKind.MarkedReviewed));
            })
            .ToArray();
    }

    public async Task<IReadOnlyList<ReviewActivityRecord>> GetRecentReviewsAsync(
        int limit = 10,
        CancellationToken cancellationToken = default)
    {
        IReadOnlyList<ReviewActivityRecord> records =
            await store.LoadAsync(cancellationToken);
        return records
            .Where(record => record.Kind is
                ReviewActivityKind.MarkedReviewed or
                ReviewActivityKind.Completed)
            .GroupBy(record => record.SessionKey, StringComparer.Ordinal)
            .Select(group => group
                .OrderByDescending(record => record.AtUtc)
                .First())
            .OrderByDescending(record => record.AtUtc)
            .Take(limit)
            .ToArray();
    }

    private Task RecordAsync(
        ReviewSessionState state,
        ReviewActivityKind kind,
        string id,
        CancellationToken cancellationToken,
        int commentDelta = 0) =>
        store.RecordAsync(
            new ReviewActivityRecord
            {
                Id = id,
                SessionKey = state.Details.Reference.SessionKey,
                Provider = state.Details.Reference.Provider,
                Repository = state.Details.Reference.Repository,
                PullRequestNumber = state.Details.Reference.Number,
                PullRequestTitle = state.Details.Title,
                Kind = kind,
                AtUtc = DateTimeOffset.UtcNow,
                CommentDelta = commentDelta
            },
            cancellationToken);
}
