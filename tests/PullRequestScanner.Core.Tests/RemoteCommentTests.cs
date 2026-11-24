using System.Net;
using System.Net.Http.Headers;
using System.Text;
using System.Text.Json;
using PullRequestScanner.Core;
using PullRequestScanner.Core.Authentication;
using PullRequestScanner.Core.Exceptions;
using PullRequestScanner.Core.Models;
using PullRequestScanner.Core.Providers;

namespace PullRequestScanner.Core.Tests;

public sealed class RemoteCommentTests
{
    [Fact]
    public async Task ConvertsUnauthorizedProviderResponseIntoAuthenticationRequest()
    {
        var handler = new RouteHttpMessageHandler(_ => new HttpResponseMessage(
            HttpStatusCode.Unauthorized)
        {
            Content = new StringContent(
                """{"message":"Bad credentials"}""",
                Encoding.UTF8,
                "application/json")
        });
        var client = new GitHubReviewProviderClient(CreateProviderHttp(handler));
        PullRequestReference reference = CreatePullRequest().Reference;

        ReviewAuthenticationException exception =
            await Assert.ThrowsAsync<ReviewAuthenticationException>(
                () => client.GetPullRequestAsync(reference));

        Assert.Equal(reference, exception.Reference);
        Assert.Contains("gh auth login", exception.Message, StringComparison.Ordinal);
    }

    [Fact]
    public async Task PublishesCommentsWithApiBearerCredentialAfterGitAccess()
    {
        AuthenticationHeaderValue? observedHeader = null;
        var handler = new RouteHttpMessageHandler(request =>
        {
            observedHeader = request.Headers.Authorization;
            return JsonResponse("""{"id":99}""");
        });
        var providerHttp = new ReviewProviderHttp(
            new HttpClient(handler),
            new FixedCredentialProvider(
                new ReviewCredential("Bearer", "api-token")));
        var client = new GitHubReviewProviderClient(providerHttp);
        PullRequestDetails pullRequest = CreatePullRequest();
        var comment = new ReviewComment
        {
            FilePath = "src/Foo.cs",
            LineNumber = 8,
            Side = ReviewCommentSide.Right,
            Body = "Authenticated comment."
        };

        string remoteId = await client.PublishCommentAsync(
            pullRequest,
            comment);

        Assert.Equal("99", remoteId);
        Assert.NotNull(observedHeader);
        Assert.Equal("Bearer", observedHeader.Scheme);
        Assert.Equal("api-token", observedHeader.Parameter);
    }

    [Fact]
    public async Task DeletesGitHubPublishedCommentWithRemoteId()
    {
        HttpMethod? method = null;
        Uri? requestUri = null;
        var handler = new RouteHttpMessageHandler(request =>
        {
            method = request.Method;
            requestUri = request.RequestUri;
            return new HttpResponseMessage(HttpStatusCode.NoContent);
        });
        var client = new GitHubReviewProviderClient(
            CreateProviderHttp(handler));
        PullRequestDetails pullRequest = CreatePullRequest();

        await client.DeleteCommentAsync(
            pullRequest,
            new ReviewComment
            {
                RemoteId = "99",
                Status = ReviewCommentStatus.Published
            });

        Assert.Equal(HttpMethod.Delete, method);
        Assert.EndsWith(
            "/repos/owner/repo/pulls/comments/99",
            requestUri!.AbsolutePath,
            StringComparison.Ordinal);
    }

    [Fact]
    public async Task DeletesAzureDevOpsPublishedCommentByThreadAndCommentId()
    {
        HttpMethod? method = null;
        Uri? requestUri = null;
        var handler = new RouteHttpMessageHandler(request =>
        {
            method = request.Method;
            requestUri = request.RequestUri;
            return new HttpResponseMessage(HttpStatusCode.OK);
        });
        var client = new AzureDevOpsReviewProviderClient(
            CreateProviderHttp(handler));
        PullRequestDetails pullRequest = CreatePullRequest(
            ReviewProvider.AzureDevOps);

        await client.DeleteCommentAsync(
            pullRequest,
            new ReviewComment
            {
                RemoteId = "31:4",
                Status = ReviewCommentStatus.Published
            });

        Assert.Equal(HttpMethod.Delete, method);
        Assert.Contains(
            "/pullRequests/7/threads/31/comments/4",
            requestUri!.AbsolutePath,
            StringComparison.Ordinal);
    }

    [Fact]
    public async Task ParsesGitHubCommitIterations()
    {
        var handler = new RouteHttpMessageHandler(_ => JsonResponse(
            """
            [
              {
                "sha": "1111111111111111111111111111111111111111",
                "parents": [{ "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }],
                "author": { "login": "first" },
                "commit": {
                  "message": "First update\n\nDetails",
                  "author": { "name": "First", "date": "2026-11-20T10:00:00Z" }
                }
              },
              {
                "sha": "2222222222222222222222222222222222222222",
                "parents": [{ "sha": "1111111111111111111111111111111111111111" }],
                "author": { "login": "second" },
                "commit": {
                  "message": "Second update",
                  "author": { "name": "Second", "date": "2026-11-21T10:00:00Z" }
                }
              }
            ]
            """));
        var client = new GitHubReviewProviderClient(CreateProviderHttp(handler));

        IReadOnlyList<PullRequestIteration> iterations =
            await client.GetIterationsAsync(CreatePullRequest());

        Assert.Equal(2, iterations.Count);
        Assert.Equal(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            iterations[0].BaseCommitSha);
        Assert.Equal(
            "1111111111111111111111111111111111111111",
            iterations[1].BaseCommitSha);
        Assert.Equal("2222222222222222222222222222222222222222", iterations[1].HeadCommitSha);
        Assert.Contains("Commit 2: Second update", iterations[1].Label, StringComparison.Ordinal);
    }

    [Fact]
    public async Task ParsesAzureDevOpsIterationDeltas()
    {
        var handler = new RouteHttpMessageHandler(_ => JsonResponse(
            """
            {
              "value": [
                {
                  "id": 1,
                  "description": "Initial update",
                  "createdDate": "2026-11-20T10:00:00Z",
                  "commonRefCommit": { "commitId": "base-1" },
                  "sourceRefCommit": { "commitId": "head-1" },
                  "createdBy": { "displayName": "First" }
                },
                {
                  "id": 2,
                  "description": "Address feedback",
                  "createdDate": "2026-11-21T10:00:00Z",
                  "targetRefCommit": { "commitId": "target-2" },
                  "sourceRefCommit": { "commitId": "head-2" },
                  "createdBy": { "displayName": "Second" }
                }
              ]
            }
            """));
        var client = new AzureDevOpsReviewProviderClient(CreateProviderHttp(handler));

        IReadOnlyList<PullRequestIteration> iterations =
            await client.GetIterationsAsync(
                CreatePullRequest(ReviewProvider.AzureDevOps));

        Assert.Equal(2, iterations.Count);
        Assert.Equal("base-1", iterations[0].BaseCommitSha);
        Assert.Equal("head-1", iterations[1].BaseCommitSha);
        Assert.Equal("head-2", iterations[1].HeadCommitSha);
        Assert.Contains("Address feedback", iterations[1].Label, StringComparison.Ordinal);
    }

    [Fact]
    public async Task MarksClosedGitHubPullRequestAsMerged()
    {
        var handler = new RouteHttpMessageHandler(_ => JsonResponse(
            """
            {
              "title": "Merged PR",
              "body": "## Summary\n\nBuild-only cleanup.",
              "state": "closed",
              "merged": true,
              "updated_at": "2026-11-21T10:00:00Z",
              "user": { "login": "author" },
              "head": { "sha": "head", "ref": "feature", "repo": { "clone_url": "https://example.test/feature.git" } },
              "base": { "sha": "base", "ref": "main", "repo": { "clone_url": "https://example.test/repo.git" } }
            }
            """));
        var client = new GitHubReviewProviderClient(CreateProviderHttp(handler));

        PullRequestDetails details = await client.GetPullRequestAsync(
            CreatePullRequest().Reference);

        Assert.True(details.IsClosed);
        Assert.True(details.IsMerged);
        Assert.Equal("## Summary\n\nBuild-only cleanup.", details.Description);
    }

    [Fact]
    public async Task ParsesAzureDevOpsPullRequestDescription()
    {
        var handler = new RouteHttpMessageHandler(_ => JsonResponse(
            """
            {
              "title": "Build cleanup",
              "description": "## Summary\n\nADO description.",
              "status": "active",
              "sourceRefName": "refs/heads/feature",
              "targetRefName": "refs/heads/main",
              "createdBy": { "displayName": "Author" },
              "lastMergeSourceCommit": { "commitId": "head" },
              "lastMergeTargetCommit": { "commitId": "base" },
              "repository": { "remoteUrl": "https://example.test/repo" },
              "creationDate": "2026-11-21T10:00:00Z"
            }
            """));
        var client = new AzureDevOpsReviewProviderClient(
            CreateProviderHttp(handler));

        PullRequestDetails details = await client.GetPullRequestAsync(
            CreatePullRequest(ReviewProvider.AzureDevOps).Reference);

        Assert.Equal("## Summary\n\nADO description.", details.Description);
        Assert.Equal("Build cleanup", details.Title);
    }

    [Fact]
    public async Task ParsesGitHubInlineConversationAndReviewComments()
    {
        var handler = new RouteHttpMessageHandler(request =>
        {
            string path = request.RequestUri!.AbsolutePath;
            string json = path.EndsWith("/pulls/7/comments", StringComparison.Ordinal)
                ? """
                  [{"id":11,"body":"Check this condition.","path":"src/Foo.cs","line":18,"side":"RIGHT","user":{"login":"reviewer"},"created_at":"2026-11-21T10:00:00Z","html_url":"https://github.example/comment/11"}]
                  """
                : path.EndsWith("/issues/7/comments", StringComparison.Ordinal)
                    ? """
                      [{"id":12,"body":"General note.","user":{"login":"owner"},"created_at":"2026-11-21T10:01:00Z","html_url":"https://github.example/comment/12"}]
                      """
                    : """
                      [{"id":13,"body":"Review summary.","user":{"login":"reviewer"},"submitted_at":"2026-11-21T10:02:00Z","html_url":"https://github.example/review/13"}]
                      """;
            return JsonResponse(json);
        });
        var client = new GitHubReviewProviderClient(
            CreateProviderHttp(handler));
        PullRequestDetails pullRequest = CreatePullRequest();

        IReadOnlyList<RemoteReviewComment> comments =
            await client.GetCommentsAsync(pullRequest);

        Assert.Equal(3, comments.Count);
        RemoteReviewComment inline = Assert.Single(
            comments.Where(comment => comment.FilePath == "src/Foo.cs"));
        Assert.Equal(18, inline.LineNumber);
        Assert.Equal(ReviewCommentSide.Right, inline.Side);
        Assert.Equal("reviewer", inline.Author);
    }

    [Fact]
    public async Task ParsesAzureDevOpsInlineAndGeneralComments()
    {
        var handler = new RouteHttpMessageHandler(_ => JsonResponse(
            """
            {
              "value": [
                {
                  "id": 31,
                  "status": "active",
                  "threadContext": {
                    "filePath": "/src/Foo.cpp",
                    "rightFileStart": { "line": 42, "offset": 1 }
                  },
                  "comments": [
                    {
                      "id": 1,
                      "commentType": "text",
                      "content": "Add a lifetime check.",
                      "author": { "displayName": "Reviewer One" },
                      "publishedDate": "2026-11-21T10:00:00Z",
                      "isDeleted": false
                    }
                  ]
                },
                {
                  "id": 32,
                  "status": "fixed",
                  "comments": [
                    {
                      "id": 1,
                      "commentType": 1,
                      "content": "General feedback.",
                      "author": { "displayName": "Reviewer Two" },
                      "publishedDate": "2026-11-21T10:01:00Z",
                      "isDeleted": false
                    }
                  ]
                }
              ]
            }
            """));
        var client = new AzureDevOpsReviewProviderClient(
            CreateProviderHttp(handler));
        PullRequestDetails pullRequest = CreatePullRequest(ReviewProvider.AzureDevOps);

        IReadOnlyList<RemoteReviewComment> comments =
            await client.GetCommentsAsync(pullRequest);

        Assert.Equal(2, comments.Count);
        RemoteReviewComment inline = Assert.Single(
            comments.Where(comment => comment.FilePath == "src/Foo.cpp"));
        Assert.Equal(42, inline.LineNumber);
        Assert.False(inline.IsResolved);
        Assert.Single(comments.Where(comment => comment.IsResolved));
    }

    [Fact]
    public async Task ParsesGitHubRepliesIntoTheirParentThread()
    {
        var handler = new RouteHttpMessageHandler(request =>
        {
            string path = request.RequestUri!.AbsolutePath;
            return JsonResponse(
                path.EndsWith("/pulls/7/comments", StringComparison.Ordinal)
                    ? """
                      [
                        {
                          "id": 21,
                          "body": "Root",
                          "path": "src/Foo.cs",
                          "line": 8,
                          "side": "RIGHT",
                          "user": { "login": "reviewer" },
                          "created_at": "2026-11-21T10:00:00Z"
                        },
                        {
                          "id": 22,
                          "in_reply_to_id": 21,
                          "body": "Reply",
                          "path": "src/Foo.cs",
                          "line": 8,
                          "side": "RIGHT",
                          "user": { "login": "author" },
                          "created_at": "2026-11-21T10:01:00Z"
                        }
                      ]
                      """
                    : "[]");
        });
        var client = new GitHubReviewProviderClient(
            CreateProviderHttp(handler));

        IReadOnlyList<RemoteReviewComment> comments =
            await client.GetCommentsAsync(CreatePullRequest());

        Assert.Equal(2, comments.Count);
        RemoteReviewComment reply = Assert.Single(
            comments.Where(comment => comment.RemoteId == "22"));
        Assert.Equal("21", reply.ThreadId);
        Assert.Equal("21", reply.ParentRemoteId);
    }

    [Fact]
    public async Task PostsGitHubInlineReplyToTheParentThread()
    {
        HttpMethod? method = null;
        string? requestBody = null;
        var handler = new RouteHttpMessageHandler(async request =>
        {
            method = request.Method;
            requestBody = request.Content is null
                ? null
                : await request.Content.ReadAsStringAsync();
            return JsonResponse("""{"id":23}""");
        });
        var client = new GitHubReviewProviderClient(
            CreateProviderHttp(handler));

        string id = await client.ReplyToCommentAsync(
            CreatePullRequest(),
            new RemoteReviewComment
            {
                RemoteId = "22",
                ThreadId = "21",
                FilePath = "src/Foo.cs",
                LineNumber = 8
            },
            "Follow-up");

        Assert.Equal("23", id);
        Assert.Equal(HttpMethod.Post, method);
        using JsonDocument body = JsonDocument.Parse(requestBody!);
        Assert.Equal("Follow-up", body.RootElement.GetProperty("body").GetString());
        Assert.Equal(
            21,
            body.RootElement.GetProperty("in_reply_to_id").GetInt64());
    }

    [Fact]
    public async Task PostsAzureDevOpsReplyToTheContainingThread()
    {
        Uri? requestUri = null;
        string? requestBody = null;
        var handler = new RouteHttpMessageHandler(async request =>
        {
            requestUri = request.RequestUri;
            requestBody = request.Content is null
                ? null
                : await request.Content.ReadAsStringAsync();
            return JsonResponse("""{"id":5}""");
        });
        var client = new AzureDevOpsReviewProviderClient(
            CreateProviderHttp(handler));

        string id = await client.ReplyToCommentAsync(
            CreatePullRequest(ReviewProvider.AzureDevOps),
            new RemoteReviewComment
            {
                RemoteId = "31:4",
                ThreadId = "31",
                FilePath = "src/Foo.cpp",
                LineNumber = 42
            },
            "Follow-up");

        Assert.Equal("5", id);
        Assert.Contains(
            "/threads/31/comments",
            requestUri!.AbsolutePath,
            StringComparison.Ordinal);
        using JsonDocument body = JsonDocument.Parse(requestBody!);
        Assert.Equal(
            4,
            body.RootElement.GetProperty("parentCommentId").GetInt32());
        Assert.Equal("Follow-up", body.RootElement.GetProperty("content").GetString());
    }

    private static ReviewProviderHttp CreateProviderHttp(HttpMessageHandler handler) =>
        new(
            new HttpClient(handler),
            new NoCredentialProvider());

    private static PullRequestDetails CreatePullRequest(
        ReviewProvider provider = ReviewProvider.GitHub) =>
        new()
        {
            Reference = new PullRequestReference
            {
                Provider = provider,
                Host = provider == ReviewProvider.GitHub
                    ? "github.example"
                    : "dev.azure.com",
                Organization = "owner",
                Project = "project",
                Repository = "repo",
                Number = 7,
                WebUrl = "https://example.test/pull/7",
                ApiBaseUrl = provider == ReviewProvider.GitHub
                    ? "https://github.example/api/v3"
                    : "https://dev.azure.com/org/project"
            }
        };

    private static HttpResponseMessage JsonResponse(string json) =>
        new(HttpStatusCode.OK)
        {
            Content = new StringContent(json, Encoding.UTF8, "application/json")
        };

    private sealed class NoCredentialProvider : ICredentialProvider
    {
        public Task<ReviewCredential?> GetCredentialAsync(
            PullRequestReference reference,
            CancellationToken cancellationToken = default) =>
            Task.FromResult<ReviewCredential?>(null);
    }

    private sealed class FixedCredentialProvider(
        ReviewCredential credential) : ICredentialProvider
    {
        public Task<ReviewCredential?> GetCredentialAsync(
            PullRequestReference reference,
            CancellationToken cancellationToken = default) =>
            Task.FromResult<ReviewCredential?>(credential);
    }

    private sealed class RouteHttpMessageHandler : HttpMessageHandler
    {
        private readonly Func<HttpRequestMessage, Task<HttpResponseMessage>> response;

        public RouteHttpMessageHandler(
            Func<HttpRequestMessage, HttpResponseMessage> response) =>
            this.response = request => Task.FromResult(response(request));

        public RouteHttpMessageHandler(
            Func<HttpRequestMessage, Task<HttpResponseMessage>> response) =>
            this.response = response;

        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken) =>
            response(request);
    }
}
