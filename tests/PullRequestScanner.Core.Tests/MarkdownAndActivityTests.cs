using PullRequestScanner.Core;
using PullRequestScanner.Core.Authentication;
using PullRequestScanner.Core.Exceptions;
using PullRequestScanner.Core.Markdown;
using PullRequestScanner.Core.Models;
using PullRequestScanner.Core.Persistence;
using PullRequestScanner.Core.Repository;
using PullRequestScanner.Core.Services;

namespace PullRequestScanner.Core.Tests;

public sealed class MarkdownAndActivityTests
{
    [Fact]
    public void RendersSafeMarkdownBlockKindsAndListMarkers()
    {
        IReadOnlyList<MarkdownBlock> blocks = MarkdownParser.Parse(
            """
            # Heading

            - bullet
            2. numbered

            > quoted

            ```csharp
            var value = 1;
            ```
            """);

        Assert.Equal(
            [MarkdownBlockKind.Heading1, MarkdownBlockKind.Bullet,
                MarkdownBlockKind.Numbered, MarkdownBlockKind.Quote,
                MarkdownBlockKind.Code],
            blocks.Select(block => block.Kind));
        Assert.Equal("\u2022", blocks[1].ListMarker);
        Assert.Equal("2.", blocks[2].ListMarker);
        Assert.Equal("var value = 1;", blocks[4].Text);
        Assert.Equal(9, blocks[4].SourceLineNumber);
    }

    [Fact]
    public void ParsesExtendedHeadingsTasksAndTables()
    {
        IReadOnlyList<MarkdownBlock> blocks = MarkdownParser.Parse(
            """
            #### Details
            - [x] done
            - [ ] pending

            | Build | Result |
            |---|---|
            | x64 | Pass |
            """);

        Assert.Equal(MarkdownBlockKind.Heading4, blocks[0].Kind);
        Assert.Equal(MarkdownBlockKind.TaskChecked, blocks[1].Kind);
        Assert.Equal(MarkdownBlockKind.TaskUnchecked, blocks[2].Kind);
        Assert.Equal(MarkdownBlockKind.Table, blocks[3].Kind);
        Assert.Contains("| x64 | Pass |", blocks[3].Text, StringComparison.Ordinal);
    }

    [Fact]
    public void SanitizesRawHtmlFromRemoteComments()
    {
        string text = MarkdownParser.ToPlainText(
            "<!--status--><samp>Pipeline</samp><br>**passed**");

        Assert.Equal("Pipeline\npassed", text);
        Assert.DoesNotContain("<", text, StringComparison.Ordinal);
    }

    [Fact]
    public void PreservesSafeInlineMarkdownRuns()
    {
        MarkdownBlock block = Assert.Single(MarkdownParser.Parse(
            "Use **bold**, `code`, and [docs](https://example.test/docs)."));

        Assert.Contains(
            block.InlineRuns!,
            run => run.Kind == MarkdownInlineKind.Bold &&
                   run.Text == "bold");
        Assert.Contains(
            block.InlineRuns!,
            run => run.Kind == MarkdownInlineKind.Code &&
                   run.Text == "code");
        MarkdownInlineRun link = Assert.Single(
            block.InlineRuns!.Where(
                run => run.Kind == MarkdownInlineKind.Link));
        Assert.Equal("docs", link.Text);
        Assert.Equal("https://example.test/docs", link.NavigateUri!.AbsoluteUri);
    }

    [Fact]
    public async Task AggregatesActivityForEachDayOfTheRequestedMonth()
    {
        string root = Path.Combine(
            Path.GetTempPath(),
            "PullRequestScanner.Tests",
            Guid.NewGuid().ToString("N"));
        var store = new JsonReviewActivityStore(
            Path.Combine(root, "review-activity.json"));
        var service = new ReviewActivityService(store);
        DateTimeOffset firstDay = new(
            2026,
            11,
            12,
            12,
            0,
            0,
            TimeSpan.Zero);
        try
        {
            await store.RecordAsync(CreateRecord(
                "open-1",
                "pr-1",
                ReviewActivityKind.Opened,
                firstDay));
            await store.RecordAsync(CreateRecord(
                "open-duplicate",
                "pr-1",
                ReviewActivityKind.Completed,
                firstDay));
            await store.RecordAsync(CreateRecord(
                "comment-1",
                "pr-1",
                ReviewActivityKind.CommentPublished,
                firstDay,
                1));
            await store.RecordAsync(CreateRecord(
                "comment-2",
                "pr-1",
                ReviewActivityKind.ReplyPublished,
                firstDay,
                1));
            await store.RecordAsync(CreateRecord(
                "review-1",
                "pr-1",
                ReviewActivityKind.MarkedReviewed,
                firstDay));
            await store.RecordAsync(CreateRecord(
                "outside",
                "pr-2",
                ReviewActivityKind.Opened,
                firstDay.AddMonths(1)));

            IReadOnlyList<ReviewActivityDay> days =
                await service.GetMonthAsync(2026, 11);

            Assert.Equal(30, days.Count);
            ReviewActivityDay day = Assert.Single(
                days.Where(item => item.Date == new DateOnly(2026, 11, 12)));
            Assert.Equal(1, day.PullRequestCount);
            Assert.Equal(2, day.CommentCount);
            Assert.Equal(1, day.ReviewCount);
            Assert.All(
                days.Where(item => item.Date != day.Date),
                item =>
                {
                    Assert.Equal(0, item.PullRequestCount);
                    Assert.Equal(0, item.CommentCount);
                });

            IReadOnlyList<ReviewActivityRecord> recent =
                await service.GetRecentReviewsAsync();
            Assert.Equal("pr-1", Assert.Single(recent).SessionKey);
        }
        finally
        {
            if (Directory.Exists(root))
            {
                Directory.Delete(root, recursive: true);
            }
        }
    }

    [Fact]
    public async Task DeletesOnlyManagedDirectChildRepositories()
    {
        string root = Path.Combine(
            Path.GetTempPath(),
            "PullRequestScanner.Tests",
            Guid.NewGuid().ToString("N"));
        string cacheRoot = Path.Combine(root, "Cache");
        string managed = Path.Combine(cacheRoot, "managed");
        string outside = Path.Combine(root, "outside");
        Directory.CreateDirectory(managed);
        Directory.CreateDirectory(outside);
        var service = new LocalRepositoryService(
            new ProcessRunner(),
            new NoCredentialProvider(),
            cacheRoot,
            []);

        try
        {
            Assert.True(service.IsManagedCachePath(managed));
            Assert.False(service.IsManagedCachePath(outside));
            await Assert.ThrowsAsync<ReviewOperationException>(
                () => service.DeleteCachedRepositoryAsync(outside));
            Assert.True(Directory.Exists(outside));

            await service.DeleteCachedRepositoryAsync(managed);
            Assert.False(Directory.Exists(managed));
        }
        finally
        {
            if (Directory.Exists(root))
            {
                Directory.Delete(root, recursive: true);
            }
        }
    }

    private static ReviewActivityRecord CreateRecord(
        string id,
        string sessionKey,
        ReviewActivityKind kind,
        DateTimeOffset atUtc,
        int commentDelta = 0) =>
        new()
        {
            Id = id,
            SessionKey = sessionKey,
            Repository = "repo",
            PullRequestNumber = 7,
            Kind = kind,
            AtUtc = atUtc,
            CommentDelta = commentDelta
        };

    private sealed class NoCredentialProvider : ICredentialProvider
    {
        public Task<ReviewCredential?> GetCredentialAsync(
            PullRequestReference reference,
            CancellationToken cancellationToken = default) =>
            Task.FromResult<ReviewCredential?>(null);
    }
}
