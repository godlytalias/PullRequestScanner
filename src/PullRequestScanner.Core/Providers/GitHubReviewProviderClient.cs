using System.Globalization;
using System.Text.Json;
using PullRequestScanner.Core.Exceptions;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Providers;

public sealed class GitHubReviewProviderClient(ReviewProviderHttp providerHttp) : IReviewProviderClient
{
    public ReviewProvider Provider => ReviewProvider.GitHub;

    public async Task<PullRequestDetails> GetPullRequestAsync(
        PullRequestReference reference,
        CancellationToken cancellationToken = default)
    {
        string requestUri =
            $"{reference.ApiBaseUrl}/repos/{Uri.EscapeDataString(reference.Organization)}/" +
            $"{Uri.EscapeDataString(reference.Repository)}/pulls/{reference.Number}";

        using JsonDocument document = await providerHttp.GetJsonAsync(
            reference,
            requestUri,
            cancellationToken);
        JsonElement root = document.RootElement;

        string headSha = GetString(root.GetProperty("head"), "sha");
        string baseSha = GetString(root.GetProperty("base"), "sha");
        string state = GetString(root, "state");
        bool isMerged = GetOptionalBoolean(root, "merged") ?? false;
        string? cloneUrl = GetOptionalString(root.GetProperty("base"), "repo", "clone_url");
        cloneUrl ??= reference.CloneUrl;

        if (string.IsNullOrWhiteSpace(headSha) || string.IsNullOrWhiteSpace(baseSha))
        {
            throw new ReviewOperationException("GitHub returned a pull request without source and target commit IDs.");
        }

        return new PullRequestDetails
        {
            Reference = reference,
            Title = GetString(root, "title"),
            Description = GetString(root, "body"),
            Author = GetOptionalString(root, "user", "login") ?? string.Empty,
            State = state,
            IsClosed = isMerged ||
                       state.Equals("closed", StringComparison.OrdinalIgnoreCase),
            IsMerged = isMerged,
            SourceBranch = GetString(root.GetProperty("head"), "ref"),
            TargetBranch = GetString(root.GetProperty("base"), "ref"),
            HeadCommitSha = headSha,
            BaseCommitSha = baseSha,
            SourceFetchRef = $"refs/pull/{reference.Number}/head",
            TargetFetchRef = $"refs/heads/{GetString(root.GetProperty("base"), "ref")}",
            CloneUrl = cloneUrl,
            UpdatedAtUtc = ParseOptionalDate(root, "updated_at")
        };
    }

    public async Task<IReadOnlyList<PullRequestIteration>> GetIterationsAsync(
        PullRequestDetails pullRequest,
        CancellationToken cancellationToken = default)
    {
        PullRequestReference reference = pullRequest.Reference;
        string baseUri =
            $"{reference.ApiBaseUrl}/repos/{Uri.EscapeDataString(reference.Organization)}/" +
            $"{Uri.EscapeDataString(reference.Repository)}/pulls/{reference.Number}/commits";
        var iterations = new List<PullRequestIteration>();
        for (int page = 1; page <= 100; page++)
        {
            string separator = baseUri.Contains('?', StringComparison.Ordinal) ? "&" : "?";
            using JsonDocument document = await providerHttp.GetJsonAsync(
                reference,
                $"{baseUri}{separator}per_page=100&page={page}",
                cancellationToken);
            if (document.RootElement.ValueKind != JsonValueKind.Array)
            {
                throw new ReviewOperationException("GitHub returned an invalid pull request commits response.");
            }

            int count = 0;
            foreach (JsonElement commit in document.RootElement.EnumerateArray())
            {
                count++;
                string headSha = GetString(commit, "sha");
                if (string.IsNullOrWhiteSpace(headSha))
                {
                    continue;
                }

                int number = iterations.Count + 1;
                string parentSha = GetFirstParentSha(commit);
                string baseSha = !string.IsNullOrWhiteSpace(parentSha)
                    ? parentSha
                    : iterations.LastOrDefault()?.HeadCommitSha ??
                      pullRequest.BaseCommitSha;
                string message = GetOptionalString(commit, "commit", "message") ?? string.Empty;
                string subject = message
                    .Replace("\r", string.Empty, StringComparison.Ordinal)
                    .Split('\n', StringSplitOptions.RemoveEmptyEntries)
                    .FirstOrDefault() ?? "Commit";
                string author = GetOptionalString(commit, "author", "login") ??
                    GetOptionalString(commit, "commit", "author", "name") ??
                    string.Empty;
                DateTimeOffset createdAt = GetCommitDate(commit);
                iterations.Add(new PullRequestIteration
                {
                    Id = headSha,
                    Number = number,
                    Label = $"Commit {number}: {subject} ({headSha[..Math.Min(8, headSha.Length)]})",
                    Author = author,
                    BaseCommitSha = baseSha,
                    HeadCommitSha = headSha,
                    CreatedAtUtc = createdAt
                });
            }

            if (count < 100)
            {
                break;
            }
        }

        return iterations;
    }

    public async Task<string> PublishCommentAsync(
        PullRequestDetails pullRequest,
        ReviewComment comment,
        CancellationToken cancellationToken = default)
    {
        PullRequestReference reference = pullRequest.Reference;
        string requestUri =
            $"{reference.ApiBaseUrl}/repos/{Uri.EscapeDataString(reference.Organization)}/" +
            $"{Uri.EscapeDataString(reference.Repository)}/pulls/{reference.Number}/comments";

        var payload = new Dictionary<string, object>
        {
            ["body"] = ReviewCommentFormatter.CreatePublishedBody(ReviewProvider.GitHub, comment),
            ["commit_id"] = comment.CommitSha ?? pullRequest.HeadCommitSha,
            ["path"] = comment.FilePath.Replace('\\', '/'),
            ["side"] = comment.Side == ReviewCommentSide.Left ? "LEFT" : "RIGHT",
            ["line"] = comment.LineNumber
        };

        using JsonDocument document = await providerHttp.SendJsonAsync(
            reference,
            HttpMethod.Post,
            requestUri,
            payload,
            cancellationToken);
        return GetPropertyString(document.RootElement, "id")
            ?? throw new ReviewOperationException("GitHub published the comment but did not return its ID.");
    }

    public async Task<IReadOnlyList<RemoteReviewComment>> GetCommentsAsync(
        PullRequestDetails pullRequest,
        CancellationToken cancellationToken = default)
    {
        PullRequestReference reference = pullRequest.Reference;
        string baseUri =
            $"{reference.ApiBaseUrl}/repos/{Uri.EscapeDataString(reference.Organization)}/" +
            $"{Uri.EscapeDataString(reference.Repository)}";
        Task<List<RemoteReviewComment>> inlineTask = GetPagedCommentsAsync(
            reference,
            $"{baseUri}/pulls/{reference.Number}/comments",
            ParseInlineComment,
            cancellationToken);
        Task<List<RemoteReviewComment>> issueTask = GetPagedCommentsAsync(
            reference,
            $"{baseUri}/issues/{reference.Number}/comments",
            element => ParseGeneralComment(element, "issue-comment"),
            cancellationToken);
        Task<List<RemoteReviewComment>> reviewTask = GetPagedCommentsAsync(
            reference,
            $"{baseUri}/pulls/{reference.Number}/reviews",
            element => ParseGeneralComment(element, "review"),
            cancellationToken);

        await Task.WhenAll(inlineTask, issueTask, reviewTask);
        return inlineTask.Result
            .Concat(issueTask.Result)
            .Concat(reviewTask.Result)
            .OrderByDescending(comment => comment.CreatedAtUtc)
            .ToArray();
    }

    public async Task DeleteCommentAsync(
        PullRequestDetails pullRequest,
        ReviewComment comment,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(comment.RemoteId))
        {
            throw new ReviewOperationException(
                "This comment does not have a GitHub remote ID and can be deleted only locally.");
        }

        PullRequestReference reference = pullRequest.Reference;
        string requestUri =
            $"{reference.ApiBaseUrl}/repos/{Uri.EscapeDataString(reference.Organization)}/" +
            $"{Uri.EscapeDataString(reference.Repository)}/pulls/comments/" +
            Uri.EscapeDataString(comment.RemoteId);
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

        PullRequestReference reference = pullRequest.Reference;
        string baseUri =
            $"{reference.ApiBaseUrl}/repos/{Uri.EscapeDataString(reference.Organization)}/" +
            $"{Uri.EscapeDataString(reference.Repository)}";
        object payload;
        string requestUri;
        if (!string.IsNullOrWhiteSpace(parentComment.FilePath) &&
            parentComment.LineNumber is not null)
        {
            requestUri = $"{baseUri}/pulls/{reference.Number}/comments";
            payload = new Dictionary<string, object>
            {
                ["body"] = body,
                ["in_reply_to_id"] = long.Parse(
                    parentComment.ThreadId,
                    CultureInfo.InvariantCulture)
            };
        }
        else
        {
            requestUri = $"{baseUri}/issues/{reference.Number}/comments";
            payload = new Dictionary<string, object>
            {
                ["body"] = body
            };
        }

        using JsonDocument document = await providerHttp.SendJsonAsync(
            reference,
            HttpMethod.Post,
            requestUri,
            payload,
            cancellationToken);
        return GetPropertyString(document.RootElement, "id")
            ?? throw new ReviewOperationException(
                "GitHub posted the reply but did not return its ID.");
    }

    private async Task<List<RemoteReviewComment>> GetPagedCommentsAsync(
        PullRequestReference reference,
        string baseUri,
        Func<JsonElement, RemoteReviewComment?> parser,
        CancellationToken cancellationToken)
    {
        var comments = new List<RemoteReviewComment>();
        for (int page = 1; page <= 100; page++)
        {
            string separator = baseUri.Contains('?', StringComparison.Ordinal) ? "&" : "?";
            using JsonDocument document = await providerHttp.GetJsonAsync(
                reference,
                $"{baseUri}{separator}per_page=100&page={page}",
                cancellationToken);
            if (document.RootElement.ValueKind != JsonValueKind.Array)
            {
                throw new ReviewOperationException("GitHub returned an invalid pull request comments response.");
            }

            int count = 0;
            foreach (JsonElement element in document.RootElement.EnumerateArray())
            {
                count++;
                RemoteReviewComment? comment = parser(element);
                if (comment is not null && !string.IsNullOrWhiteSpace(comment.Body))
                {
                    comments.Add(comment);
                }
            }

            if (count < 100)
            {
                break;
            }
        }

        return comments;
    }

    private static RemoteReviewComment ParseInlineComment(JsonElement element)
    {
        string remoteId = GetPropertyString(element, "id") ?? string.Empty;
        string side = GetString(element, "side");
        string parentRemoteId = GetPropertyString(element, "in_reply_to_id") ?? string.Empty;
        return new RemoteReviewComment
        {
            Id = $"inline:{remoteId}",
            RemoteId = remoteId,
            ThreadId = string.IsNullOrWhiteSpace(parentRemoteId)
                ? remoteId
                : parentRemoteId,
            ParentRemoteId = string.IsNullOrWhiteSpace(parentRemoteId)
                ? null
                : parentRemoteId,
            Author = GetOptionalString(element, "user", "login") ?? "GitHub",
            Body = GetString(element, "body"),
            FilePath = GetString(element, "path").Replace('\\', '/'),
            LineNumber = GetOptionalInt(element, "line") ?? GetOptionalInt(element, "original_line"),
            Side = side.Equals("LEFT", StringComparison.OrdinalIgnoreCase)
                ? ReviewCommentSide.Left
                : ReviewCommentSide.Right,
            WebUrl = GetString(element, "html_url"),
            CreatedAtUtc = ParseDate(element, "created_at")
        };
    }

    private static RemoteReviewComment ParseGeneralComment(
        JsonElement element,
        string prefix)
    {
        string remoteId = GetPropertyString(element, "id") ?? string.Empty;
        return new RemoteReviewComment
        {
            Id = $"{prefix}:{remoteId}",
            RemoteId = remoteId,
            ThreadId = $"{prefix}:{remoteId}",
            Author = GetOptionalString(element, "user", "login") ?? "GitHub",
            Body = GetString(element, "body"),
            WebUrl = GetString(element, "html_url"),
            CreatedAtUtc = ParseDate(element, "submitted_at") != default
                ? ParseDate(element, "submitted_at")
                : ParseDate(element, "created_at")
        };
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

    private static DateTimeOffset? ParseOptionalDate(
        JsonElement element,
        string property)
    {
        string value = GetString(element, property);
        return DateTimeOffset.TryParse(
            value,
            CultureInfo.InvariantCulture,
            DateTimeStyles.AssumeUniversal,
            out DateTimeOffset result)
            ? result
            : null;
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

    private static bool? GetOptionalBoolean(JsonElement element, string property)
    {
        if (!element.TryGetProperty(property, out JsonElement value) ||
            value.ValueKind is not JsonValueKind.True and not JsonValueKind.False)
        {
            return null;
        }

        return value.GetBoolean();
    }

    private static string GetFirstParentSha(JsonElement commit)
    {
        if (!commit.TryGetProperty("parents", out JsonElement parents) ||
            parents.ValueKind != JsonValueKind.Array ||
            parents.GetArrayLength() == 0)
        {
            return string.Empty;
        }

        return GetString(parents[0], "sha");
    }

    private static DateTimeOffset GetCommitDate(JsonElement commit)
    {
        if (!commit.TryGetProperty("commit", out JsonElement commitValue) ||
            commitValue.ValueKind != JsonValueKind.Object ||
            !commitValue.TryGetProperty("author", out JsonElement author) ||
            author.ValueKind != JsonValueKind.Object)
        {
            return default;
        }

        return ParseOptionalDate(author, "date") ?? default;
    }
}
