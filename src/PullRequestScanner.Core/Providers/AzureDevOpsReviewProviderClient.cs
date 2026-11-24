using System.Globalization;
using System.Text.Json;
using PullRequestScanner.Core.Exceptions;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Providers;

public sealed class AzureDevOpsReviewProviderClient(ReviewProviderHttp providerHttp) : IReviewProviderClient
{
    public ReviewProvider Provider => ReviewProvider.AzureDevOps;

    public async Task<PullRequestDetails> GetPullRequestAsync(
        PullRequestReference reference,
        CancellationToken cancellationToken = default)
    {
        string requestUri =
            $"{reference.ApiBaseUrl}/_apis/git/repositories/{Uri.EscapeDataString(reference.Repository)}/" +
            $"pullRequests/{reference.Number}?api-version=7.1";

        using JsonDocument document = await providerHttp.GetJsonAsync(
            reference,
            requestUri,
            cancellationToken);
        JsonElement root = document.RootElement;
        string headSha = GetOptionalString(root, "lastMergeSourceCommit", "commitId") ?? string.Empty;
        string baseSha = GetOptionalString(root, "lastMergeTargetCommit", "commitId") ?? string.Empty;
        string state = GetString(root, "status");
        string? cloneUrl = GetOptionalString(root, "repository", "remoteUrl") ?? reference.CloneUrl;

        if (string.IsNullOrWhiteSpace(headSha) || string.IsNullOrWhiteSpace(baseSha))
        {
            throw new ReviewOperationException("Azure DevOps returned a pull request without source and target commit IDs.");
        }

        return new PullRequestDetails
        {
            Reference = reference,
            Title = GetString(root, "title"),
            Description = GetString(root, "description"),
            Author = GetOptionalString(root, "createdBy", "displayName") ?? string.Empty,
            State = state,
            IsClosed = state.Equals("completed", StringComparison.OrdinalIgnoreCase) ||
                       state.Equals("abandoned", StringComparison.OrdinalIgnoreCase),
            IsMerged = state.Equals("completed", StringComparison.OrdinalIgnoreCase),
            SourceBranch = GetString(root, "sourceRefName"),
            TargetBranch = GetString(root, "targetRefName"),
            HeadCommitSha = headSha,
            BaseCommitSha = baseSha,
            SourceFetchRef = GetString(root, "sourceRefName"),
            TargetFetchRef = GetString(root, "targetRefName"),
            CloneUrl = cloneUrl,
            UpdatedAtUtc = ParseDate(root, "closedDate") != default
                ? ParseDate(root, "closedDate")
                : ParseDate(root, "creationDate")
        };
    }

    public async Task<IReadOnlyList<PullRequestIteration>> GetIterationsAsync(
        PullRequestDetails pullRequest,
        CancellationToken cancellationToken = default)
    {
        PullRequestReference reference = pullRequest.Reference;
        string requestUri =
            $"{reference.ApiBaseUrl}/_apis/git/repositories/{Uri.EscapeDataString(reference.Repository)}/" +
            $"pullRequests/{reference.Number}/iterations?includeCommits=true&api-version=7.1";
        using JsonDocument document = await providerHttp.GetJsonAsync(
            reference,
            requestUri,
            cancellationToken);
        if (!document.RootElement.TryGetProperty("value", out JsonElement values) ||
            values.ValueKind != JsonValueKind.Array)
        {
            throw new ReviewOperationException("Azure DevOps returned an invalid pull request iterations response.");
        }

        var iterations = new List<PullRequestIteration>();
        foreach (JsonElement element in values.EnumerateArray())
        {
            string headSha = GetOptionalString(element, "sourceRefCommit", "commitId") ?? string.Empty;
            if (string.IsNullOrWhiteSpace(headSha))
            {
                continue;
            }

            int number = GetOptionalInt(element, "id") ?? iterations.Count + 1;
            string baseSha = iterations.LastOrDefault()?.HeadCommitSha ??
                GetOptionalString(element, "commonRefCommit", "commitId") ??
                GetOptionalString(element, "targetRefCommit", "commitId") ??
                pullRequest.BaseCommitSha;
            string description = GetString(element, "description");
            string label = string.IsNullOrWhiteSpace(description)
                ? $"Iteration {number}"
                : $"Iteration {number}: {description}";
            iterations.Add(new PullRequestIteration
            {
                Id = number.ToString(CultureInfo.InvariantCulture),
                Number = number,
                Label = label,
                Author = GetOptionalString(element, "createdBy", "displayName") ??
                    GetOptionalString(element, "updatedBy", "displayName") ??
                    string.Empty,
                BaseCommitSha = baseSha,
                HeadCommitSha = headSha,
                CreatedAtUtc = ParseDate(element, "createdDate")
            });
        }

        return iterations
            .OrderBy(iteration => iteration.Number)
            .ToArray();
    }

    public async Task<string> PublishCommentAsync(
        PullRequestDetails pullRequest,
        ReviewComment comment,
        CancellationToken cancellationToken = default)
    {
        PullRequestReference reference = pullRequest.Reference;
        string requestUri =
            $"{reference.ApiBaseUrl}/_apis/git/repositories/{Uri.EscapeDataString(reference.Repository)}/" +
            $"pullRequests/{reference.Number}/threads?api-version=7.1";
        string side = comment.Side == ReviewCommentSide.Left ? "leftFile" : "rightFile";
        var position = new Dictionary<string, object>
        {
            ["line"] = comment.LineNumber,
            ["offset"] = 1
        };
        var context = new Dictionary<string, object>
        {
            ["filePath"] = "/" + comment.FilePath.Replace('\\', '/').TrimStart('/'),
            [$"{side}Start"] = position,
            [$"{side}End"] = position
        };
        var payload = new Dictionary<string, object>
        {
            ["comments"] = new[]
            {
                new Dictionary<string, object>
                {
                    ["parentCommentId"] = 0,
                    ["content"] = ReviewCommentFormatter.CreatePublishedBody(ReviewProvider.AzureDevOps, comment),
                    ["commentType"] = 1
                }
            },
            ["status"] = 1,
            ["threadContext"] = context
        };

        using JsonDocument document = await providerHttp.SendJsonAsync(
            reference,
            HttpMethod.Post,
            requestUri,
            payload,
            cancellationToken);
        string threadId = GetPropertyString(document.RootElement, "id")
            ?? throw new ReviewOperationException("Azure DevOps created the thread but did not return its ID.");
        string commentId = document.RootElement.TryGetProperty(
            "comments",
            out JsonElement comments) &&
            comments.ValueKind == JsonValueKind.Array &&
            comments.GetArrayLength() > 0
            ? GetPropertyString(comments[0], "id") ?? "0"
            : "0";
        return $"{threadId}:{commentId}";
    }

    public async Task<IReadOnlyList<RemoteReviewComment>> GetCommentsAsync(
        PullRequestDetails pullRequest,
        CancellationToken cancellationToken = default)
    {
        PullRequestReference reference = pullRequest.Reference;
        string requestUri =
            $"{reference.ApiBaseUrl}/_apis/git/repositories/{Uri.EscapeDataString(reference.Repository)}/" +
            $"pullRequests/{reference.Number}/threads?$top=1000&api-version=7.1";
        using JsonDocument document = await providerHttp.GetJsonAsync(
            reference,
            requestUri,
            cancellationToken);
        if (!document.RootElement.TryGetProperty("value", out JsonElement threads) ||
            threads.ValueKind != JsonValueKind.Array)
        {
            throw new ReviewOperationException("Azure DevOps returned an invalid pull request threads response.");
        }

        var remoteComments = new List<RemoteReviewComment>();
        foreach (JsonElement thread in threads.EnumerateArray())
        {
            string threadId = GetPropertyString(thread, "id") ?? string.Empty;
            string threadStatus = GetString(thread, "status");
            bool isResolved = !threadStatus.Equals(
                "active",
                StringComparison.OrdinalIgnoreCase) &&
                !threadStatus.Equals(
                    "pending",
                    StringComparison.OrdinalIgnoreCase) &&
                !threadStatus.Equals(
                    "unknown",
                    StringComparison.OrdinalIgnoreCase);
            (string filePath, int? lineNumber, ReviewCommentSide? side) = ParseThreadContext(thread);
            if (!thread.TryGetProperty("comments", out JsonElement comments) ||
                comments.ValueKind != JsonValueKind.Array)
            {
                continue;
            }

            foreach (JsonElement comment in comments.EnumerateArray())
            {
                string body = GetString(comment, "content");
                bool deleted = GetOptionalBoolean(comment, "isDeleted") ?? false;
                if (string.IsNullOrWhiteSpace(body) ||
                    deleted ||
                    !IsTextComment(comment))
                {
                    continue;
                }

                string commentId = GetPropertyString(comment, "id") ?? string.Empty;
                remoteComments.Add(new RemoteReviewComment
                {
                    Id = $"thread:{threadId}:comment:{commentId}",
                    RemoteId = $"{threadId}:{commentId}",
                    ThreadId = threadId,
                    ParentRemoteId = FormatParentRemoteId(
                        threadId,
                        GetPropertyString(comment, "parentCommentId")),
                    Author = GetOptionalString(comment, "author", "displayName") ?? "Azure DevOps",
                    Body = body,
                    FilePath = filePath,
                    LineNumber = lineNumber,
                    Side = side,
                    IsResolved = isResolved,
                    WebUrl = reference.WebUrl,
                    CreatedAtUtc = ParseDate(comment, "publishedDate")
                });
            }
        }

        return remoteComments
            .OrderByDescending(comment => comment.CreatedAtUtc)
            .ToArray();
    }

    public async Task DeleteCommentAsync(
        PullRequestDetails pullRequest,
        ReviewComment comment,
        CancellationToken cancellationToken = default)
    {
        string[] idParts = (comment.RemoteId ?? string.Empty).Split(
            ':',
            StringSplitOptions.RemoveEmptyEntries);
        if (idParts.Length != 2)
        {
            throw new ReviewOperationException(
                "This Azure DevOps comment does not have both thread and comment IDs.");
        }

        PullRequestReference reference = pullRequest.Reference;
        string requestUri =
            $"{reference.ApiBaseUrl}/_apis/git/repositories/{Uri.EscapeDataString(reference.Repository)}/" +
            $"pullRequests/{reference.Number}/threads/{Uri.EscapeDataString(idParts[0])}/" +
            $"comments/{Uri.EscapeDataString(idParts[1])}?api-version=7.1";
        await providerHttp.DeleteAsync(
            reference,
            requestUri,
            cancellationToken);
    }

    public async Task<string> ReplyToCommentAsync(
        PullRequestDetails pullRequest,
        RemoteReviewComment parentComment,
        string body,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(body))
        {
            throw new ArgumentException("Reply text is required.", nameof(body));
        }

        if (string.IsNullOrWhiteSpace(parentComment.ThreadId))
        {
            throw new ReviewOperationException(
                "This Azure DevOps comment is not associated with a replyable thread.");
        }

        PullRequestReference reference = pullRequest.Reference;
        string requestUri =
            $"{reference.ApiBaseUrl}/_apis/git/repositories/{Uri.EscapeDataString(reference.Repository)}/" +
            $"pullRequests/{reference.Number}/threads/" +
            $"{Uri.EscapeDataString(parentComment.ThreadId)}/comments?api-version=7.1";
        var payload = new Dictionary<string, object>
        {
            ["parentCommentId"] = ParseCommentId(parentComment.RemoteId),
            ["content"] = body,
            ["commentType"] = 1
        };
        using JsonDocument document = await providerHttp.SendJsonAsync(
            reference,
            HttpMethod.Post,
            requestUri,
            payload,
            cancellationToken);
        return GetPropertyString(document.RootElement, "id")
            ?? throw new ReviewOperationException(
                "Azure DevOps posted the reply but did not return its ID.");
    }

    private static int ParseCommentId(string? remoteId)
    {
        string value = (remoteId ?? string.Empty).Split(':').LastOrDefault() ?? string.Empty;
        return int.TryParse(value, out int result) ? result : 0;
    }

    private static string? FormatParentRemoteId(
        string threadId,
        string? parentId) =>
        !string.IsNullOrWhiteSpace(parentId) && parentId != "0"
            ? $"{threadId}:{parentId}"
            : null;

    private static (string FilePath, int? LineNumber, ReviewCommentSide? Side) ParseThreadContext(
        JsonElement thread)
    {
        if (!thread.TryGetProperty("threadContext", out JsonElement context) ||
            context.ValueKind == JsonValueKind.Null)
        {
            return (string.Empty, null, null);
        }

        string path = GetString(context, "filePath").TrimStart('/');
        int? rightLine = GetNestedInt(context, "rightFileStart", "line");
        if (rightLine is not null)
        {
            return (path, rightLine, ReviewCommentSide.Right);
        }

        int? leftLine = GetNestedInt(context, "leftFileStart", "line");
        return (path, leftLine, leftLine is null ? null : ReviewCommentSide.Left);
    }

    private static bool IsTextComment(JsonElement comment)
    {
        if (!comment.TryGetProperty("commentType", out JsonElement type))
        {
            return true;
        }

        return type.ValueKind switch
        {
            JsonValueKind.String => type.GetString()!.Equals(
                "text",
                StringComparison.OrdinalIgnoreCase),
            JsonValueKind.Number => type.TryGetInt32(out int value) && value == 1,
            _ => false
        };
    }

    private static int? GetNestedInt(
        JsonElement element,
        string objectProperty,
        string valueProperty)
    {
        if (!element.TryGetProperty(objectProperty, out JsonElement nested) ||
            nested.ValueKind == JsonValueKind.Null ||
            !nested.TryGetProperty(valueProperty, out JsonElement value) ||
            value.ValueKind != JsonValueKind.Number ||
            !value.TryGetInt32(out int result))
        {
            return null;
        }

        return result;
    }

    private static DateTimeOffset ParseDate(JsonElement element, string property)
    {
        string value = GetString(element, property);
        return DateTimeOffset.TryParse(
            value,
            CultureInfo.InvariantCulture,
            DateTimeStyles.AssumeUniversal,
            out DateTimeOffset result)
            ? result
            : default;
    }

    private static bool? GetOptionalBoolean(JsonElement element, string property)
    {
        if (!element.TryGetProperty(property, out JsonElement value) ||
            value.ValueKind is not JsonValueKind.True and not JsonValueKind.False)
        {
            return null;
        }

        return value.GetBoolean();
    }

    private static int? GetOptionalInt(JsonElement element, string property)
    {
        if (!element.TryGetProperty(property, out JsonElement value) ||
            value.ValueKind != JsonValueKind.Number ||
            !value.TryGetInt32(out int result))
        {
            return null;
        }

        return result;
    }

    private static string GetString(JsonElement element, string property) =>
        element.TryGetProperty(property, out JsonElement value) && value.ValueKind == JsonValueKind.String
            ? value.GetString() ?? string.Empty
            : string.Empty;

    private static string? GetOptionalString(JsonElement element, params string[] properties)
    {
        JsonElement current = element;
        foreach (string property in properties)
        {
            if (current.ValueKind == JsonValueKind.Null ||
                !current.TryGetProperty(property, out current))
            {
                return null;
            }
        }

        return current.ValueKind == JsonValueKind.String ? current.GetString() : null;
    }

    private static string? GetPropertyString(JsonElement element, string property) =>
        element.TryGetProperty(property, out JsonElement value)
            ? value.ValueKind switch
            {
                JsonValueKind.String => value.GetString(),
                JsonValueKind.Number => value.GetRawText(),
                _ => null
            }
            : null;
}
