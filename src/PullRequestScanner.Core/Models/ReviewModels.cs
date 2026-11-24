namespace PullRequestScanner.Core.Models;

public enum ReviewProvider
{
    GitHub,
    AzureDevOps
}

public enum ChangedFileStatus
{
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged
}

public enum DiffLineKind
{
    Context,
    Added,
    Deleted,
    Hunk
}

public enum ReviewCommentSide
{
    Left,
    Right
}

public enum ReviewCommentStatus
{
    Pending,
    Published,
    Failed
}

public sealed class PullRequestReference
{
    public ReviewProvider Provider { get; init; }
    public string Host { get; init; } = string.Empty;
    public string Organization { get; init; } = string.Empty;
    public string Project { get; init; } = string.Empty;
    public string Repository { get; init; } = string.Empty;
    public int Number { get; init; }
    public string WebUrl { get; init; } = string.Empty;
    public string ApiBaseUrl { get; init; } = string.Empty;
    public string CloneUrl { get; init; } = string.Empty;
    public string SessionKey { get; init; } = string.Empty;
}

public sealed class PullRequestDetails
{
    public PullRequestReference Reference { get; init; } = new();
    public string Title { get; init; } = string.Empty;
    public string Description { get; init; } = string.Empty;
    public string Author { get; init; } = string.Empty;
    public string State { get; init; } = string.Empty;
    public bool IsClosed { get; init; }
    public bool IsMerged { get; init; }
    public string SourceBranch { get; init; } = string.Empty;
    public string TargetBranch { get; init; } = string.Empty;
    public string HeadCommitSha { get; init; } = string.Empty;
    public string BaseCommitSha { get; init; } = string.Empty;
    public string SourceFetchRef { get; init; } = string.Empty;
    public string TargetFetchRef { get; init; } = string.Empty;
    public string CloneUrl { get; init; } = string.Empty;
    public DateTimeOffset? UpdatedAtUtc { get; init; }
}

public sealed class PullRequestIteration
{
    public const string AllChangesId = "all-changes";

    public string Id { get; init; } = string.Empty;
    public int Number { get; init; }
    public string Label { get; init; } = string.Empty;
    public string Author { get; init; } = string.Empty;
    public string BaseCommitSha { get; init; } = string.Empty;
    public string HeadCommitSha { get; init; } = string.Empty;
    public DateTimeOffset CreatedAtUtc { get; init; }
    public bool IsAllChanges { get; init; }

    public override string ToString() => Label;
}

public sealed class ChangedFile
{
    public string Path { get; init; } = string.Empty;
    public string? PreviousPath { get; init; }
    public ChangedFileStatus Status { get; init; }
}

public sealed class DiffLine
{
    public DiffLineKind Kind { get; init; }
    public string FilePath { get; init; } = string.Empty;
    public int? OldLineNumber { get; init; }
    public int? NewLineNumber { get; init; }
    public string Text { get; init; } = string.Empty;
}

public sealed class DiffBlock
{
    public string FilePath { get; init; } = string.Empty;
    public string? PreviousPath { get; init; }
    public IReadOnlyList<DiffLine> Lines { get; init; } = [];
}

public sealed record DiffLinePair(
    DiffLine? LeftLine,
    DiffLine? RightLine,
    DiffLine? HunkLine)
{
    public bool IsHunk => HunkLine is not null;
}

public sealed class ReviewComment
{
    public string Id { get; init; } = Guid.NewGuid().ToString("N");
    public string FilePath { get; init; } = string.Empty;
    public int LineNumber { get; init; }
    public ReviewCommentSide Side { get; init; }
    public string Body { get; set; } = string.Empty;
    public string SourceLineText { get; set; } = string.Empty;
    public int? SelectionStart { get; set; }
    public int SelectionLength { get; set; }
    public string SelectedText { get; set; } = string.Empty;
    public string SuggestedReplacement { get; set; } = string.Empty;
    public string? IterationId { get; set; }
    public string? CommitSha { get; set; }
    public ReviewCommentStatus Status { get; set; } = ReviewCommentStatus.Pending;
    public string? RemoteId { get; set; }
    public string? Error { get; set; }
    public DateTimeOffset CreatedAtUtc { get; init; } = DateTimeOffset.UtcNow;
    public DateTimeOffset? PublishedAtUtc { get; set; }
}

public sealed class RemoteReviewComment
{
    public string Id { get; init; } = string.Empty;
    public string RemoteId { get; init; } = string.Empty;
    public string ThreadId { get; init; } = string.Empty;
    public string? ParentRemoteId { get; init; }
    public string Author { get; init; } = string.Empty;
    public string Body { get; init; } = string.Empty;
    public string FilePath { get; init; } = string.Empty;
    public int? LineNumber { get; init; }
    public ReviewCommentSide? Side { get; init; }
    public bool IsResolved { get; init; }
    public string WebUrl { get; init; } = string.Empty;
    public DateTimeOffset CreatedAtUtc { get; init; }
}

public sealed class ReviewSessionState
{
    public int SchemaVersion { get; set; } = 5;
    public PullRequestDetails Details { get; set; } = new();
    public string? LocalRepositoryPath { get; set; }
    public bool ReusedExistingRepository { get; set; }
    public string Notes { get; set; } = string.Empty;
    public List<string> ViewedFiles { get; set; } = [];
    public List<ChangedFile> ChangedFiles { get; set; } = [];
    public List<ReviewComment> Comments { get; set; } = [];
    public List<RemoteReviewComment> RemoteComments { get; set; } = [];
    public List<PullRequestIteration> Iterations { get; set; } = [];
    public string? SelectedFilePath { get; set; }
    public string? SelectedIterationId { get; set; }
    public string? LastReviewedIterationId { get; set; }
    public DateTimeOffset? LastReviewedAtUtc { get; set; }
    public bool IsArchived { get; set; }
    public DateTimeOffset LastOpenedUtc { get; set; } = DateTimeOffset.UtcNow;
    public DateTimeOffset? CompletedAtUtc { get; set; }
}

public enum MarkdownBlockKind
{
    Paragraph,
    Heading1,
    Heading2,
    Heading3,
    Bullet,
    Numbered,
    Quote,
    Code,
    HorizontalRule,
    Heading4,
    Heading5,
    Heading6,
    TaskChecked,
    TaskUnchecked,
    Table
}

public sealed record MarkdownBlock(
    MarkdownBlockKind Kind,
    string Text,
    int SourceLineNumber,
    int SourceLineCount = 1,
    string? Language = null,
    string? ListMarker = null,
    IReadOnlyList<MarkdownInlineRun>? InlineRuns = null);

public enum MarkdownInlineKind
{
    Text,
    Bold,
    Code,
    Link
}

public sealed record MarkdownInlineRun(
    MarkdownInlineKind Kind,
    string Text,
    Uri? NavigateUri = null);

public enum ReviewActivityKind
{
    Opened,
    CommentPublished,
    ReplyPublished,
    MarkedReviewed,
    Completed
}

public sealed class ReviewActivityRecord
{
    public string Id { get; init; } = Guid.NewGuid().ToString("N");
    public string SessionKey { get; init; } = string.Empty;
    public ReviewProvider Provider { get; init; }
    public string Repository { get; init; } = string.Empty;
    public int PullRequestNumber { get; init; }
    public string PullRequestTitle { get; init; } = string.Empty;
    public ReviewActivityKind Kind { get; init; }
    public DateTimeOffset AtUtc { get; init; } = DateTimeOffset.UtcNow;
    public int CommentDelta { get; init; }
}

public sealed class ReviewActivityStoreState
{
    public int SchemaVersion { get; set; } = 1;
    public List<ReviewActivityRecord> Records { get; set; } = [];
}

public sealed record SymbolLocation(
    string Symbol,
    string FilePath,
    int LineNumber,
    string LineText,
    int Score);
