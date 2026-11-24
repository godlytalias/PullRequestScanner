using System.Text;
using PullRequestScanner.Core.Authentication;
using PullRequestScanner.Core.Models;
using PullRequestScanner.Core.Navigation;
using PullRequestScanner.Core.Persistence;
using PullRequestScanner.Core.Providers;
using PullRequestScanner.Core.Repository;
using PullRequestScanner.Core.Services;

namespace PullRequestScanner.Core.Tests;

public sealed class StateAndNavigationTests
{
    [Theory]
    [InlineData("git@github.com:octo-org/code-review-demo.git", "github.com/octo-org/code-review-demo")]
    [InlineData("https://dev.azure.com/contoso/Project/_git/repository", "dev.azure.com/contoso/project/_git/repository")]
    [InlineData("https://user@github.com/OWNER/Repo.git", "github.com/owner/repo")]
    public void NormalizesCommonGitRemotes(string input, string expected)
    {
        Assert.Equal(expected, RemoteUrlNormalizer.Normalize(input));
    }

    [Fact]
    public void ExtractsFunctionCandidatesAndSkipsControlKeywords()
    {
        IReadOnlyList<string> candidates = SymbolCandidateExtractor.Extract(
            "if (CanReview(item)) ReviewItem(item); else RetryAsync();");

        Assert.Equal(["CanReview", "ReviewItem", "RetryAsync"], candidates);
    }

    [Fact]
    public void PreservesQualificationAndMarksStandardLibraryCallsExternal()
    {
        IReadOnlyList<SymbolCandidate> candidates = SymbolCandidateExtractor.ExtractCandidates(
            "return std::clamp(value, low, high) + LocalHelper(value);");

        Assert.Collection(
            candidates,
            clamp =>
            {
                Assert.Equal("clamp", clamp.Name);
                Assert.Equal("std::clamp", clamp.DisplayName);
                Assert.Equal(7, clamp.ColumnIndex);
                Assert.True(clamp.IsLikelyExternal);
            },
            helper =>
            {
                Assert.Equal("LocalHelper", helper.Name);
                Assert.False(helper.IsLikelyExternal);
            });
    }

    [Fact]
    public void IgnoresCallsInsideCommentsAndStringsAndControlKeywords()
    {
        IReadOnlyList<string> candidates = SymbolCandidateExtractor.Extract(
            """// Example header;""");
        IReadOnlyList<string> codeCandidates = SymbolCandidateExtractor.Extract(
            """string text = "QuotedCall()"; ASSERT(Check()); ActualCall();""");

        Assert.Empty(candidates);
        Assert.Equal(["Check", "ActualCall"], codeCandidates);
    }

    [Fact]
    public void FormatsGitHubSuggestionFromPartialLineSelection()
    {
        var comment = new ReviewComment
        {
            Body = "Use the bounded value.",
            SourceLineText = "    return value + 1;",
            SelectionStart = 11,
            SelectionLength = 5,
            SelectedText = "value",
            Side = ReviewCommentSide.Right,
            SuggestedReplacement = "bounded"
        };

        string body = ReviewCommentFormatter.CreatePublishedBody(
            ReviewProvider.GitHub,
            comment);

        Assert.Contains("Selected code:\n> value", body, StringComparison.Ordinal);
        Assert.Contains(
            "```suggestion\n    return bounded + 1;\n```",
            body,
            StringComparison.Ordinal);
    }

    [Fact]
    public async Task RoundTripsReviewStateAtomically()
    {
        string root = Path.Combine(Path.GetTempPath(), "PullRequestScanner.Tests", Guid.NewGuid().ToString("N"));
        var store = new JsonReviewStateStore(root);
        DateTimeOffset lastOpenedUtc =
            DateTimeOffset.Parse("2026-11-20T08:30:00Z");
        var state = new ReviewSessionState
        {
            Details = new PullRequestDetails
            {
                Reference = new PullRequestReference
                {
                    Provider = ReviewProvider.GitHub,
                    SessionKey = "github:host/owner/repo#7",
                    Repository = "repo"
                },
                Title = "Test review"
            },
            LastOpenedUtc = lastOpenedUtc,
            Notes = "Remember to check lifetime.",
            Comments =
            [
                new ReviewComment
                {
                    FilePath = "src/Foo.cs",
                    LineNumber = 7,
                    Side = ReviewCommentSide.Right,
                    Body = "Please guard the null case."
                }
            ]
        };

        try
        {
            await store.SaveAsync(state);
            ReviewSessionState restored = Assert.IsType<ReviewSessionState>(
                await store.LoadAsync(state.Details.Reference.SessionKey));

            Assert.Equal(state.Notes, restored.Notes);
            Assert.Equal(lastOpenedUtc, state.LastOpenedUtc);
            Assert.Equal(lastOpenedUtc, restored.LastOpenedUtc);
            Assert.Equal(ReviewProvider.GitHub, restored.Details.Reference.Provider);
            Assert.Equal("Please guard the null case.", Assert.Single(restored.Comments).Body);
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
    public void FormatsConversationCommentWithoutTouchingLocalCommentFallback()
    {
        Assert.Equal(
            ReviewCommentLocation.Conversation,
            ReviewCommentLocation.Format(string.Empty, null));
    }

    [Fact]
    public void SelectsIterationRangeAndFallsBackToAllChanges()
    {
        var state = new ReviewSessionState
        {
            Details = new PullRequestDetails
            {
                BaseCommitSha = "base",
                HeadCommitSha = "head"
            },
            Iterations =
            [
                new PullRequestIteration
                {
                    Id = "1",
                    BaseCommitSha = "iteration-base",
                    HeadCommitSha = "iteration-head"
                }
            ],
            SelectedIterationId = "1"
        };

        PullRequestReviewRange range = ReviewSessionService.GetActiveRange(state);
        Assert.Equal("iteration-base", range.BaseCommitSha);
        Assert.Equal("iteration-head", range.HeadCommitSha);

        state.SelectedIterationId = "missing";
        range = ReviewSessionService.GetActiveRange(state);
        Assert.Equal("base", range.BaseCommitSha);
        Assert.Equal("head", range.HeadCommitSha);
    }

    [Fact]
    public void FormatsGitHubTokenForGitSmartHttp()
    {
        string header = GitAuthentication.FormatAuthorizationHeader(
            ReviewProvider.GitHub,
            new ReviewCredential("Bearer", "github-token"));
        Assert.Equal(
            "Authorization: Basic " +
            Convert.ToBase64String(
                Encoding.ASCII.GetBytes("x-access-token:github-token")),
            header);
    }

    [Fact]
    public void KeepsAzureDevOpsGitAuthorizationScheme()
    {
        var credential = new ReviewCredential("Basic", "ado-encoded-pat");
        Assert.Equal(
            "Authorization: Basic ado-encoded-pat",
            GitAuthentication.FormatAuthorizationHeader(
                ReviewProvider.AzureDevOps,
                credential));
    }

    [Theory]
    [InlineData("remote: invalid credentials\nfatal: Authentication failed", true)]
    [InlineData("fatal: could not read Username", true)]
    [InlineData("fatal: repository not found", true)]
    [InlineData("fatal: couldn't find remote ref refs/pull/1/head", false)]
    public void DetectsGitAuthenticationFailures(
        string stderr,
        bool expected)
    {
        Assert.Equal(
            expected,
            GitAuthentication.IndicatesAuthenticationFailure(
                new ProcessResult(128, string.Empty, stderr)));
    }

    [Fact]
    public async Task RoundTripsInlineRemoteCommentLineAnchor()
    {
        string root = Path.Combine(
            Path.GetTempPath(),
            "PullRequestScanner.Tests",
            Guid.NewGuid().ToString("N"));
        var store = new JsonReviewStateStore(root);
        string sessionKey = "github:host/owner/repo#remote-anchor";
        var state = new ReviewSessionState
        {
            Details = new PullRequestDetails
            {
                Reference = new PullRequestReference
                {
                    SessionKey = sessionKey
                }
            },
            RemoteComments =
            [
                new RemoteReviewComment
                {
                    Id = "remote",
                    RemoteId = "1",
                    FilePath = "src/Smoke.cs",
                    LineNumber = 3,
                    Side = ReviewCommentSide.Right,
                    Author = "Reviewer",
                    Body = "Inline"
                }
            ]
        };

        try
        {
            await store.SaveAsync(state);
            ReviewSessionState restored = Assert.IsType<ReviewSessionState>(
                await store.LoadAsync(sessionKey));
            RemoteReviewComment comment = Assert.Single(restored.RemoteComments);

            Assert.Equal("src/Smoke.cs", comment.FilePath);
            Assert.Equal(3, comment.LineNumber);
            Assert.Equal(ReviewCommentSide.Right, comment.Side);
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
    public async Task MigratesVersionTwoStateWithIterationCollections()
    {
        string root = Path.Combine(
            Path.GetTempPath(),
            "PullRequestScanner.Tests",
            Guid.NewGuid().ToString("N"));
        var store = new JsonReviewStateStore(root);
        string sessionKey = "github:host/owner/legacy#3";
        Directory.CreateDirectory(root);
        await File.WriteAllTextAsync(
            store.GetPath(sessionKey),
            """
            {
              "SchemaVersion": 2,
              "Details": {
                "Reference": {
                  "Provider": "GitHub",
                  "SessionKey": "github:host/owner/legacy#3",
                  "Repository": "legacy"
                }
              },
              "Notes": "Legacy review"
            }
            """);

        try
        {
            ReviewSessionState state = Assert.IsType<ReviewSessionState>(
                await store.LoadAsync(sessionKey));

            Assert.Equal(5, state.SchemaVersion);
            Assert.NotNull(state.Iterations);
            Assert.Empty(state.Iterations);
            Assert.NotNull(state.RemoteComments);
        }
        finally
        {
            Directory.Delete(root, recursive: true);
        }
    }

    [Theory]
    [InlineData(true, true, "comment", "", false, true)]
    [InlineData(true, true, "", "replacement", false, true)]
    [InlineData(true, true, "", "", false, false)]
    [InlineData(false, true, "comment", "", false, false)]
    [InlineData(true, false, "comment", "", false, false)]
    [InlineData(true, true, "comment", "", true, false)]
    public void ComputesCommentSaveReadiness(
        bool hasSession,
        bool hasCommentableLine,
        string comment,
        string suggestion,
        bool isBusy,
        bool expected)
    {
        Assert.Equal(
            expected,
            ReviewActionState.CanSaveComment(
                hasSession,
                hasCommentableLine,
                comment,
                suggestion,
                isBusy));
    }

    [Theory]
    [InlineData(1, false, true)]
    [InlineData(0, false, false)]
    [InlineData(1, true, false)]
    public void ComputesPublishReadiness(
        int pendingCount,
        bool isBusy,
        bool expected)
    {
        Assert.Equal(
            expected,
            ReviewActionState.CanPublishComments(pendingCount, isBusy));
    }
}
