using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using PullRequestScanner.Core.Markdown;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.App.ViewModels;

public sealed partial class ReviewCommentViewModel : ObservableObject
{
    private readonly ReviewComment? localComment;
    private readonly RemoteReviewComment? remoteComment;
    private readonly Func<RemoteReviewComment, Task>? navigateToComment;
    private readonly Func<RemoteReviewComment, string, Task>? replyToComment;
    private readonly Func<ReviewComment, Task>? deleteComment;
    private readonly Func<ReviewComment, string, string, Task<bool>>? editComment;

    public ReviewCommentViewModel(
        ReviewComment comment,
        Func<ReviewComment, Task>? deleteComment = null,
        Func<ReviewComment, string, string, Task<bool>>? editComment = null)
    {
        ArgumentNullException.ThrowIfNull(comment);
        localComment = comment;
        this.deleteComment = deleteComment;
        this.editComment = editComment;
        DeleteCommand = new AsyncRelayCommand(
            () => deleteComment?.Invoke(comment) ?? Task.CompletedTask,
            () => CanDelete);
        EditCommand = new RelayCommand(
            BeginEdit,
            () => CanEdit);
        SaveEditCommand = new AsyncRelayCommand(
            SaveEditAsync,
            () => IsEditing &&
                  (!string.IsNullOrWhiteSpace(EditBody) ||
                   !string.IsNullOrWhiteSpace(EditSuggestion)));
    }

    public ReviewCommentViewModel(
        RemoteReviewComment comment,
        Func<RemoteReviewComment, Task>? navigateToComment = null,
        Func<RemoteReviewComment, string, Task>? replyToComment = null,
        bool showJumpToLine = true)
    {
        ArgumentNullException.ThrowIfNull(comment);
        remoteComment = comment;
        this.navigateToComment = navigateToComment;
        this.replyToComment = replyToComment;
        ShowJumpToLine = showJumpToLine;
        NavigateCommand = new AsyncRelayCommand(
            () => navigateToComment?.Invoke(comment) ?? Task.CompletedTask,
            () => CanNavigateInApp);
        ReplyCommand = new AsyncRelayCommand(
            ReplyAsync,
            () => CanReply &&
                  !string.IsNullOrWhiteSpace(ReplyText));
    }

    public string Id => remoteComment is not null
        ? remoteComment.Id
        : localComment?.Id ?? string.Empty;
    public string Author => remoteComment is not null
        ? remoteComment.Author
        : "You";
    public string Location => remoteComment is not null
        ? ReviewCommentLocation.Format(
            remoteComment.FilePath,
            remoteComment.LineNumber)
        : ReviewCommentLocation.Format(
            localComment?.FilePath,
            localComment?.LineNumber);

    public string Body => remoteComment is not null
        ? remoteComment.Body ?? string.Empty
        : localComment?.Body ?? string.Empty;
    public string DisplayBody => MarkdownParser.ToPlainText(Body);
    public bool ShowReadOnlyBody => IsNotEditing;
    public string SelectedText => localComment?.SelectedText ?? string.Empty;
    public bool HasSelection => !string.IsNullOrWhiteSpace(SelectedText);
    public bool ShowSelection => IsNotEditing && HasSelection;
    public string SuggestedReplacement => localComment?.SuggestedReplacement ?? string.Empty;
    public bool HasSuggestion => !string.IsNullOrWhiteSpace(SuggestedReplacement);
    public bool ShowSuggestion => IsNotEditing && HasSuggestion;
    public string Error => localComment?.Error ?? string.Empty;
    public bool HasError => !string.IsNullOrWhiteSpace(Error);
    public bool ShowError => IsNotEditing && HasError;
    public string Status => remoteComment is not null
        ? remoteComment.IsResolved
            ? "Resolved"
            : "Active"
        : localComment?.Status switch
        {
            ReviewCommentStatus.Pending => "Draft",
            ReviewCommentStatus.Failed => "Failed",
            ReviewCommentStatus.Published => "Published",
            _ => "Draft"
        };
    public bool IsDraft => localComment?.Status is
        ReviewCommentStatus.Pending or ReviewCommentStatus.Failed;
    public bool IsEditing => editing;
    public bool IsNotEditing => !IsEditing;
    public bool IsResolved => remoteComment?.IsResolved == true;
    public bool IsNotResolved => !IsResolved;
    public bool IsActiveStatus => IsRemote && !IsResolved;
    public bool IsPublishedStatus => localComment?.Status ==
        ReviewCommentStatus.Published;
    public bool IsDraftStatus => localComment?.Status ==
        ReviewCommentStatus.Pending;
    public bool IsFailedStatus => localComment?.Status ==
        ReviewCommentStatus.Failed;
    public bool IsBot => Author.Contains(
        "[bot]",
        StringComparison.OrdinalIgnoreCase) ||
        Author.Contains("azure-pipelines", StringComparison.OrdinalIgnoreCase);
    public string FilePath => remoteComment?.FilePath ??
        localComment?.FilePath ??
        string.Empty;
    public bool IsRemote => remoteComment is not null;
    public ReviewCommentSide? Side => remoteComment is not null
        ? remoteComment.Side
        : localComment?.Side;
    public string WebUrl => remoteComment is not null
        ? remoteComment.WebUrl ?? string.Empty
        : string.Empty;
    public Uri? WebUri => Uri.TryCreate(WebUrl, UriKind.Absolute, out Uri? uri)
        ? uri
        : null;
    public bool CanNavigateInApp => remoteComment is not null &&
        !string.IsNullOrWhiteSpace(remoteComment.FilePath) &&
        remoteComment.LineNumber is not null &&
        remoteComment.Side is not null &&
        navigateToComment is not null;
    public bool CanOpenInProvider => WebUri is not null && !CanNavigateInApp;
    public bool ShowStatusText => !CanNavigateInApp && !CanOpenInProvider;
    public bool ShowJumpToLine { get; }
    public bool ShowInAppJump => ShowJumpToLine && CanNavigateInApp;
    public bool ShowProviderLink => WebUri is not null && !CanNavigateInApp;
    public IAsyncRelayCommand NavigateCommand { get; } =
        new AsyncRelayCommand(() => Task.CompletedTask, () => false);
    public bool IsPending => localComment?.Status is
        ReviewCommentStatus.Pending or ReviewCommentStatus.Failed;
    public bool CanDelete => localComment is not null && deleteComment is not null;
    public bool CanEdit => localComment is not null &&
        IsDraft &&
        editComment is not null;
    public IAsyncRelayCommand DeleteCommand { get; } =
        new AsyncRelayCommand(() => Task.CompletedTask, () => false);
    public IRelayCommand EditCommand { get; } =
        new RelayCommand(() => { }, () => false);
    public IAsyncRelayCommand SaveEditCommand { get; } =
        new AsyncRelayCommand(() => Task.CompletedTask, () => false);
    public IAsyncRelayCommand ReplyCommand { get; } =
        new AsyncRelayCommand(() => Task.CompletedTask, () => false);
    public ObservableCollection<ReviewCommentViewModel> Replies { get; } = [];
    public bool HasReplies => Replies.Count > 0;
    public string ThreadSummary => HasReplies
        ? $"{Replies.Count + 1} comment(s)"
        : string.Empty;
    public string ExpansionGlyph => IsExpanded
        ? "\uE70D"
        : "\uE70E";
    public bool CanReply => remoteComment is not null &&
        replyToComment is not null;
    public bool ShowReplyButton => CanReply &&
        !IsEditing &&
        !IsReplyComposerVisible;
    public bool ShowReplyComposer => CanReply &&
        !IsEditing &&
        IsReplyComposerVisible;

    [ObservableProperty]
    public partial bool IsExpanded { get; set; } = true;

    [ObservableProperty]
    public partial string ReplyText { get; set; } = string.Empty;

    [ObservableProperty]
    public partial bool IsReplyComposerVisible { get; set; }

    [ObservableProperty]
    public partial string EditBody { get; set; } = string.Empty;

    [ObservableProperty]
    public partial string EditSuggestion { get; set; } = string.Empty;

    private bool editing;

    public void SetReplies(IEnumerable<ReviewCommentViewModel> replies)
    {
        Replies.Clear();
        foreach (ReviewCommentViewModel reply in replies)
        {
            Replies.Add(reply);
        }

        OnPropertyChanged(nameof(HasReplies));
        OnPropertyChanged(nameof(ThreadSummary));
    }

    private async Task ReplyAsync()
    {
        if (remoteComment is null ||
            replyToComment is null ||
            string.IsNullOrWhiteSpace(ReplyText))
        {
            return;
        }

        string body = ReplyText.Trim();
        await replyToComment(remoteComment, body);
        ReplyText = string.Empty;
        IsReplyComposerVisible = false;
    }

    [RelayCommand]
    private void BeginReply() =>
        IsReplyComposerVisible = true;

    [RelayCommand]
    private void CancelReply()
    {
        ReplyText = string.Empty;
        IsReplyComposerVisible = false;
    }

    [RelayCommand]
    private void CancelEdit()
    {
        SetEditing(false);
        EditBody = string.Empty;
        EditSuggestion = string.Empty;
    }

    private void BeginEdit()
    {
        EditBody = Body;
        EditSuggestion = SuggestedReplacement;
        SetEditing(true);
    }

    private async Task SaveEditAsync()
    {
        if (localComment is null ||
            editComment is null)
        {
            return;
        }

        string body = EditBody.Trim();
        string suggestion = EditSuggestion.TrimEnd();
        if (string.IsNullOrWhiteSpace(body) &&
            string.IsNullOrWhiteSpace(suggestion))
        {
            return;
        }

        if (string.IsNullOrWhiteSpace(body))
        {
            body = "Suggested change.";
        }

        if (await editComment(localComment, body, suggestion))
        {
            SetEditing(false);
        }
    }

    [RelayCommand]
    private void ToggleExpanded()
    {
        IsExpanded = !IsExpanded;
    }

    partial void OnReplyTextChanged(string value) =>
        ReplyCommand.NotifyCanExecuteChanged();

    partial void OnIsExpandedChanged(bool value) =>
        OnPropertyChanged(nameof(ExpansionGlyph));

    partial void OnIsReplyComposerVisibleChanged(bool value)
    {
        OnPropertyChanged(nameof(ShowReplyButton));
        OnPropertyChanged(nameof(ShowReplyComposer));
    }

    partial void OnEditBodyChanged(string value) =>
        SaveEditCommand.NotifyCanExecuteChanged();

    partial void OnEditSuggestionChanged(string value) =>
        SaveEditCommand.NotifyCanExecuteChanged();

    private void SetEditing(bool value)
    {
        editing = value;
        OnPropertyChanged(nameof(IsEditing));
        OnPropertyChanged(nameof(IsNotEditing));
        OnPropertyChanged(nameof(ShowSelection));
        OnPropertyChanged(nameof(ShowSuggestion));
        OnPropertyChanged(nameof(ShowError));
        OnPropertyChanged(nameof(ShowReadOnlyBody));
        OnPropertyChanged(nameof(ShowReplyButton));
        OnPropertyChanged(nameof(ShowReplyComposer));
        SaveEditCommand.NotifyCanExecuteChanged();
    }

    public bool MatchesFile(string path) =>
        string.IsNullOrWhiteSpace(path) ||
        string.Equals(
            FilePath.Replace('\\', '/'),
            path.Replace('\\', '/'),
            StringComparison.OrdinalIgnoreCase);

    public override string ToString() =>
        $"{Status} comment by {Author} at {Location}";
}
