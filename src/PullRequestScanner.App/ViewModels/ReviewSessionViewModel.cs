using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using PullRequestScanner.Core.Diff;
using PullRequestScanner.Core.Exceptions;
using PullRequestScanner.Core.Models;
using PullRequestScanner.Core.Navigation;
using PullRequestScanner.Core.Markdown;
using PullRequestScanner.Core.Services;

namespace PullRequestScanner.App.ViewModels;

public sealed partial class ReviewSessionViewModel : ObservableObject
{
    private readonly ReviewSessionService sessionService;
    private ReviewSessionState? state;
    private CancellationTokenSource? loadCancellation;
    private string selectedPath = string.Empty;
    private bool selectedPathIsFolder;
    private readonly HashSet<int> collapsedFunctionStarts = [];
    private IReadOnlyList<CodeFoldRegion> functionRegions = [];
    private List<DiffLineViewModel> searchMatches = [];
    private int currentSearchIndex = -1;
    private bool isFocusedCommentMode;
    private bool isViewingDefinition;
    private string focusedSuggestionSeed = string.Empty;
    private bool suppressNotesChanges;
    private CancellationTokenSource? notesSaveDebounce;

    public event Action<DiffLineViewModel>? ScrollToLineRequested;
    public event Action<DiffLineViewModel>? CommentComposerRequested;
    public event EventHandler? ReviewStateChanged;
    public event Action<PullRequestReference, Func<Task>>? AuthenticationRequired;
    public event EventHandler? DiffMarkersChanged;

    public ReviewSessionViewModel(ReviewSessionService sessionService)
    {
        this.sessionService = sessionService;
        IsEmpty = true;
    }

    partial void OnIsMarkdownFormattedChanged(bool value)
    {
        NotifyMarkdownPresentationChanged();
    }

    [RelayCommand]
    private void ToggleMarkdownFormat()
    {
        if (IsMarkdownFile)
        {
            IsMarkdownFormatted = !IsMarkdownFormatted;
        }
    }

    public ReviewSessionViewModel(
        ReviewSessionService sessionService,
        ReviewSessionState state)
    {
        this.sessionService = sessionService;
        ReplaceState(state);
    }

    [ObservableProperty]
    public partial bool IsEmpty { get; set; }

    [ObservableProperty]
    public partial bool IsBusy { get; set; }

    [ObservableProperty]
    public partial string ErrorMessage { get; set; } = string.Empty;

    [ObservableProperty]
    public partial string StatusMessage { get; set; } = string.Empty;

    [ObservableProperty]
    public partial string Notes { get; set; } = string.Empty;

    [ObservableProperty]
    public partial string NotesSaveStatus { get; set; } = "Saved on this device.";

    [ObservableProperty]
    public partial string DraftComment { get; set; } = string.Empty;

    [ObservableProperty]
    public partial string DraftSuggestion { get; set; } = string.Empty;

    [ObservableProperty]
    public partial string SelectedCodeFragment { get; set; } = string.Empty;

    [ObservableProperty]
    public partial string DiffHeading { get; set; } = string.Empty;

    [ObservableProperty]
    public partial int ContextLines { get; set; } = 3;

    [ObservableProperty]
    public partial FileTreeNodeViewModel? SelectedTreeNode { get; set; }

    [ObservableProperty]
    public partial DiffLineViewModel? SelectedDiffLine { get; set; }

    [ObservableProperty]
    public partial PullRequestIteration? SelectedIteration { get; set; }

    [ObservableProperty]
    public partial int DiffLayoutModeIndex { get; set; }

    [ObservableProperty]
    public partial string SearchQuery { get; set; } = string.Empty;

    [ObservableProperty]
    public partial string FileFilter { get; set; } = string.Empty;

    [ObservableProperty]
    public partial int InspectorTabIndex { get; set; }

    [ObservableProperty]
    public partial bool ShowOnlyCurrentFileComments { get; set; }

    [ObservableProperty]
    public partial bool HideBotComments { get; set; }

    [ObservableProperty]
    public partial bool IsMarkdownFormatted { get; set; } = true;

    public bool IsInlineDiff => DiffLayoutModeIndex == 0;
    public bool IsSideBySideDiff => DiffLayoutModeIndex == 1;
    public bool IsMarkdownFile => selectedMarkdownFile;
    public bool IsFormattedMarkdownVisible => IsMarkdownFile && IsMarkdownFormatted;
    public bool IsRawCodeVisible => !IsFormattedMarkdownVisible;
    public string MarkdownToggleLabel => IsMarkdownFormatted
        ? "View source"
        : "View formatted";
    public bool IsInlineRawCodeVisible => IsInlineDiff && IsRawCodeVisible;
    public bool IsSideBySideRawCodeVisible => IsSideBySideDiff && IsRawCodeVisible;
    public bool AreDiffMarkersVisible => HasDiffMarkers && IsRawCodeVisible;
    public bool HasDescription => !string.IsNullOrWhiteSpace(
        state?.Details.Description);
    public bool HasActiveReview => !IsEmpty;
    public bool HasError => !string.IsNullOrWhiteSpace(ErrorMessage);
    public string NavigationTitle => IsEmpty
        ? string.Empty
        : $"{state!.Details.Reference.Repository} #{state.Details.Reference.Number}";
    public string NavigationPrimaryTitle => IsEmpty
        ? string.Empty
        : $"#{state!.Details.Reference.Number} · {state.Details.Title}";
    public string NavigationSecondaryTitle => IsEmpty
        ? string.Empty
        : $"{state!.Details.Reference.Host} · {state.Details.Reference.Repository}";
    public int PullRequestNumber => IsEmpty
        ? 0
        : state!.Details.Reference.Number;
    public string NavigationTooltip => IsEmpty
        ? string.Empty
        : $"{NavigationPrimaryTitle}\n{NavigationSecondaryTitle}";
    public string Title => IsEmpty ? string.Empty : state!.Details.Title;
    public string NavigationSubtitle => Title;
    public override string ToString() => NavigationPrimaryTitle;
    public string Author => IsEmpty ? string.Empty : state!.Details.Author;
    public string ProviderLabel => IsEmpty
        ? string.Empty
        : state!.Details.Reference.Provider == ReviewProvider.GitHub
            ? "GitHub"
            : "Azure DevOps";
    public string PullRequestUrl => IsEmpty ? string.Empty : state!.Details.Reference.WebUrl;
    public Uri? PullRequestUri => Uri.TryCreate(
        PullRequestUrl,
        UriKind.Absolute,
        out Uri? uri)
        ? uri
        : null;
    public string RepositoryLocation => IsEmpty
        ? string.Empty
        : state!.LocalRepositoryPath ?? state.Details.CloneUrl;
    public string RepositorySourceLabel => IsEmpty
        ? string.Empty
        : state!.ReusedExistingRepository ? "Reusing local checkout" : "Using app cache";
    public string SelectedLineDescription => SelectedDiffLine is null
        ? string.Empty
        : $"{SelectedDiffLine.FilePath}:" +
          (SelectedDiffLine.CommentSide == ReviewCommentSide.Left
              ? SelectedDiffLine.Line.OldLineNumber
              : SelectedDiffLine.Line.NewLineNumber ?? SelectedDiffLine.Line.OldLineNumber) +
          (string.IsNullOrWhiteSpace(SelectedCodeFragment)
              ? string.Empty
              : $" - selected: {SelectedCodeFragment}");
    public int ChangeCount => DiffLines.Count(line => line.IsChange);
    public int ChangedFileCount => state?.ChangedFiles.Count ?? 0;
    public string ChangedFilesSummary => IsEmpty
        ? string.Empty
        : $"Changed files ({ChangedFileCount})";
    public string ChangePosition => GetChangePosition();
    public string WorkspaceSubtitle => IsEmpty
        ? string.Empty
        : $"{Author} · {DiffHeading}";
    public string WorkspaceStatus => $"{ChangePosition} · {LastReviewedLabel}";
    public bool HasSelectedCodeFragment => !string.IsNullOrWhiteSpace(SelectedCodeFragment);
    public string FocusedCodePreview => SelectedDiffLine?.CodeText ?? string.Empty;
    public bool HasFocusedCodePreview =>
        !string.IsNullOrWhiteSpace(FocusedCodeContext);
    public string FocusedCodeContext => GetFocusedCodeContext();
    public bool IsOpen => !IsEmpty &&
        state!.IsArchived == false &&
        IsProviderTerminalState(state.Details) == false;
    public bool IsPastReview => !IsEmpty && !IsOpen;
    public bool HasNewIteration => !IsEmpty &&
        IsOpen &&
        !string.IsNullOrWhiteSpace(state!.LastReviewedIterationId) &&
        state.Iterations.LastOrDefault() is PullRequestIteration latest &&
        !latest.Id.Equals(
            state.LastReviewedIterationId,
            StringComparison.Ordinal);
    public string NewIterationLabel => HasNewIteration
        ? $"New iteration"
        : string.Empty;
    public string ReviewStatusLabel => IsEmpty
        ? string.Empty
        : state!.Details.IsMerged
            ? "Merged"
            : IsProviderTerminalState(state.Details)
                ? "Closed"
                : state.IsArchived
                    ? "Reviewed"
                    : state.LastReviewedIterationId is null
                        ? "Not marked reviewed"
                        : "Up to date";
    public string LastReviewedLabel => state?.LastReviewedAtUtc is DateTimeOffset reviewedAt
        ? $"Last reviewed {reviewedAt.LocalDateTime:g}"
        : "Not marked reviewed";
    public bool CanMarkReviewed => state?.Iterations.Count > 0;
    public int PendingCommentCount => IsEmpty
        ? 0
        : state!.Comments.Count(comment =>
            comment.Status is ReviewCommentStatus.Pending or ReviewCommentStatus.Failed);
    public bool HasPendingComments => PendingCommentCount > 0;
    public string PendingCommentSummary => PendingCommentCount switch
    {
        0 => "No comments waiting to publish.",
        1 => "1 comment waiting to publish.",
        _ => $"{PendingCommentCount} comments waiting to publish."
    };
    public string PublishButtonLabel => PendingCommentCount switch
    {
        0 => "No comments to submit",
        1 => "Submit 1 comment",
        _ => $"Submit {PendingCommentCount} comments"
    };
    public bool CanSaveCommentLocally => CanQueueComment();
    public bool CanPublishPendingComments => CanPublishComments();
    public bool HasFunctionFolds => functionRegions.Count > 0 && !selectedPathIsFolder;
    public bool HasDiffMarkers => DiffMarkers.Count > 0;
    public bool CanSearchFile => !selectedPathIsFolder &&
        DiffLines.Count > 0;
    public bool HasSearchMatches => searchMatches.Count > 0;
    public bool HasSearchQuery => !string.IsNullOrWhiteSpace(SearchQuery);
    public bool IsFolderSelection => selectedPathIsFolder;
    public bool IsViewingDefinition => isViewingDefinition;
    public bool IsCommentsInspectorVisible => InspectorTabIndex == 0;
    public bool IsOverviewInspectorVisible => InspectorTabIndex == 1;
    public bool IsNotesInspectorVisible => InspectorTabIndex == 2;
    public int SearchMatchCount => searchMatches.Count;
    public string SearchPosition => searchMatches.Count == 0
        ? "No matches"
        : $"{currentSearchIndex + 1} / {searchMatches.Count}";
    public bool CanMoveSearchPrevious => searchMatches.Count > 0;
    public bool CanMoveSearchNext => searchMatches.Count > 0;
    public ObservableCollection<FileTreeNodeViewModel> FileRoots { get; } = [];
    public ObservableCollection<DiffLineViewModel> DiffLines { get; } = [];
    public ObservableCollection<DiffLineViewModel> VisibleDiffLines { get; } = [];
    public ObservableCollection<DiffRowViewModel> DiffRows { get; } = [];
    public ObservableCollection<DiffChangeMarkerViewModel> DiffMarkers { get; } = [];
    public ObservableCollection<MarkdownBlockViewModel> MarkdownBlocks { get; } = [];
    public ObservableCollection<MarkdownBlockViewModel> DescriptionMarkdownBlocks { get; } = [];
    public ObservableCollection<ReviewCommentViewModel> Comments { get; } = [];
    public ObservableCollection<ReviewCommentViewModel> VisibleComments { get; } = [];
    public ObservableCollection<ReviewCommentViewModel> RemoteComments { get; } = [];
    public ObservableCollection<ReviewCommentViewModel> VisibleRemoteComments { get; } = [];
    public ObservableCollection<PullRequestIteration> Iterations { get; } = [];
    public IReadOnlyList<int> ContextOptions { get; } = [3, 5, 10, 20, 40];

    public ReviewSessionState? State => state;
    private bool selectedMarkdownFile;

    public void ReplaceState(ReviewSessionState newState)
    {
        state = newState;
        IsEmpty = false;
        suppressNotesChanges = true;
        Notes = state.Notes;
        suppressNotesChanges = false;
        NotesSaveStatus = "Saved on this device.";
        ErrorMessage = string.Empty;
        StatusMessage = string.Empty;
        DiffLines.Clear();
        VisibleDiffLines.Clear();
        DiffRows.Clear();
        DiffHeading = "Select a file to view changes";
        collapsedFunctionStarts.Clear();
        functionRegions = [];
        ClearSearch();
        MarkdownBlocks.Clear();
        selectedMarkdownFile = false;
        RebuildDescriptionMarkdown();
        RebuildIterations();
        RebuildFiles();
        RefreshComments();
        NotifyIdentityChanged();
    }

    public void SetFocusedCommentMode(bool enabled)
    {
        isFocusedCommentMode = enabled;
        foreach (DiffLineViewModel line in DiffLines)
        {
            line.SetFocusedCommentMode(enabled);
        }

        RebuildDiffRows();
    }

    public async Task SelectTreeNodeAsync(FileTreeNodeViewModel node)
    {
        node.ExpandAncestors();
        if (node.IsFolder)
        {
            node.IsExpanded = true;
        }

        SelectedTreeNode = node;
        selectedPath = node.Path;
        selectedPathIsFolder = node.IsFolder;
        isViewingDefinition = false;
        OnPropertyChanged(nameof(IsFolderSelection));
        RebuildFilteredComments();
        OnPropertyChanged(nameof(IsViewingDefinition));
        if (node.IsFile && state is not null)
        {
            state.SelectedFilePath = node.Path;
            if (!state.ViewedFiles.Contains(node.Path, StringComparer.OrdinalIgnoreCase))
            {
                state.ViewedFiles.Add(node.Path);
            }

            node.SetViewed(true);
            await SaveStateAsync();
        }

        await ReloadDiffAsync();
        OnPropertyChanged(nameof(CanSearchFile));
        OnPropertyChanged(nameof(WorkspaceSubtitle));
        OnPropertyChanged(nameof(WorkspaceStatus));
    }

    public async Task EnsureInitialFileSelectedAsync()
    {
        if (state is null ||
            !string.IsNullOrWhiteSpace(selectedPath) ||
            state.ChangedFiles.Count == 0)
        {
            return;
        }

        FileTreeNodeViewModel? node = null;
        if (!string.IsNullOrWhiteSpace(state.SelectedFilePath))
        {
            node = FindFileNode(FileRoots, state.SelectedFilePath);
        }

        if (node is null && state.ViewedFiles.Count > 0)
        {
            node = state.ViewedFiles
                .AsEnumerable()
                .Reverse()
                .Select(path => FindFileNode(FileRoots, path))
                .FirstOrDefault(found => found?.IsFile == true);
        }

        node ??= FindFirstFile(FileRoots);
        if (node is not null)
        {
            await SelectTreeNodeAsync(node);
        }
    }

    [RelayCommand]
    private void ShowCommentsInspector()
    {
        InspectorTabIndex = 0;
        NotifyInspectorTabsChanged();
    }

    [RelayCommand]
    private void ShowOverviewInspector()
    {
        InspectorTabIndex = 1;
        NotifyInspectorTabsChanged();
    }

    [RelayCommand]
    private void ShowNotesInspector()
    {
        InspectorTabIndex = 2;
        NotifyInspectorTabsChanged();
    }

    [RelayCommand]
    private async Task ReturnToDiffAsync()
    {
        if (!isViewingDefinition)
        {
            return;
        }

        isViewingDefinition = false;
        OnPropertyChanged(nameof(IsViewingDefinition));
        await ReloadDiffAsync();
    }

    [RelayCommand]
    private void CollapseAllFunctions()
    {
        collapsedFunctionStarts.Clear();
        foreach (CodeFoldRegion region in functionRegions)
        {
            collapsedFunctionStarts.Add(region.StartIndex);
        }

        RebuildVisibleDiffLines();
        RebuildDiffRows();
        RebuildDiffMarkers();
        StatusMessage = $"Collapsed {functionRegions.Count} function(s).";
    }

    [RelayCommand]
    private void ExpandAllFunctions()
    {
        collapsedFunctionStarts.Clear();
        RebuildVisibleDiffLines();
        RebuildDiffRows();
        RebuildDiffMarkers();
        StatusMessage = $"Expanded {functionRegions.Count} function(s).";
    }

    [RelayCommand]
    private void CollapseAllComments()
    {
        foreach (ReviewCommentViewModel comment in Comments.Concat(RemoteComments))
        {
            comment.IsExpanded = false;
        }

        StatusMessage = "Collapsed review comments.";
    }

    [RelayCommand]
    private void ExpandAllComments()
    {
        foreach (ReviewCommentViewModel comment in Comments.Concat(RemoteComments))
        {
            comment.IsExpanded = true;
        }

        StatusMessage = "Expanded review comments.";
    }

    [RelayCommand]
    private void PreviousSearchResult()
    {
        NavigateSearch(-1);
    }

    [RelayCommand]
    private void NextSearchResult()
    {
        NavigateSearch(1);
    }

    public async Task ChangeContextAsync(int contextLines)
    {
        ContextLines = contextLines;
        if (!string.IsNullOrWhiteSpace(selectedPath))
        {
            await ReloadDiffAsync();
        }
    }

    public void SelectLine(DiffLineViewModel line)
    {
        if (!ReferenceEquals(SelectedDiffLine, line))
        {
            SelectedDiffLine = line;
        }
    }

    public async Task SelectIterationAsync(PullRequestIteration iteration)
    {
        if (state is null)
        {
            return;
        }

        string? iterationId = iteration.IsAllChanges
            ? null
            : iteration.Id;
        if (state.SelectedIterationId == iterationId)
        {
            return;
        }

        await RunBusyAsync(async () =>
        {
            state = await sessionService.SelectIterationAsync(
                state,
                iterationId);
            selectedPath = string.Empty;
            selectedPathIsFolder = false;
            OnPropertyChanged(nameof(IsFolderSelection));
            SelectedTreeNode = null;
            DiffLines.Clear();
            VisibleDiffLines.Clear();
            DiffMarkers.Clear();
            MarkdownBlocks.Clear();
            selectedMarkdownFile = false;
            NotifyMarkdownPresentationChanged();
            RebuildFiles();
            RebuildIterations();
            RebuildDiffRows();
            StatusMessage = iteration.Label;
        });
    }

    public async Task RefreshSummaryAsync()
    {
        if (state is null)
        {
            return;
        }

        try
        {
            state = await sessionService.RefreshSummaryAsync(state);
            RebuildDescriptionMarkdown();
            RebuildIterations();
            NotifyIdentityChanged();
            ReviewStateChanged?.Invoke(this, EventArgs.Empty);
        }
        catch (ReviewOperationException exception)
        {
            ErrorMessage = $"Could not refresh {NavigationTitle}: {exception.Message}";
        }
        catch (HttpRequestException exception)
        {
            ErrorMessage = $"Could not refresh {NavigationTitle}: {exception.Message}";
        }
    }

    public async Task<bool> ArchiveAsync()
    {
        if (state is null)
        {
            return false;
        }

        bool previous = state.IsArchived;
        state.IsArchived = true;
        if (!await SaveStateAsync())
        {
            state.IsArchived = previous;
            return false;
        }

        NotifyIdentityChanged();
        ReviewStateChanged?.Invoke(this, EventArgs.Empty);
        return true;
    }

    public async Task<bool> ReopenAsync()
    {
        if (state is null || IsProviderTerminalState(state.Details))
        {
            return false;
        }

        bool previous = state.IsArchived;
        state.IsArchived = false;
        if (!await SaveStateAsync())
        {
            state.IsArchived = previous;
            return false;
        }

        NotifyIdentityChanged();
        ReviewStateChanged?.Invoke(this, EventArgs.Empty);
        return true;
    }

    public async Task<bool> CompleteAsync()
    {
        if (state is null)
        {
            return false;
        }

        bool completed = await RunBusyAsync(
            () => sessionService.CompleteReviewAsync(state));
        if (completed)
        {
            ReviewStateChanged?.Invoke(this, EventArgs.Empty);
        }

        return completed;
    }

    [RelayCommand]
    private async Task SaveNotesAsync()
    {
        if (state is null)
        {
            return;
        }

        state.Notes = Notes;
        if (await SaveStateAsync())
        {
            NotesSaveStatus = "Saved on this device.";
            StatusMessage = "Notes saved on this device.";
        }
    }

    [RelayCommand]
    private async Task AddCommentAsync()
    {
        if (state is null || SelectedDiffLine is null)
        {
            return;
        }

        string body = DraftComment.Trim();
        string suggestion = DraftSuggestion.TrimEnd();
        DiffLine line = SelectedDiffLine.Line;
        int? lineNumber = SelectedDiffLine.CommentSide == ReviewCommentSide.Left
            ? line.OldLineNumber
            : line.NewLineNumber ?? line.OldLineNumber;
        if ((string.IsNullOrWhiteSpace(body) && string.IsNullOrWhiteSpace(suggestion)) ||
            lineNumber is null)
        {
            return;
        }

        if (string.IsNullOrWhiteSpace(body))
        {
            body = "Suggested change.";
        }

        ReviewComment comment = new()
        {
            FilePath = line.FilePath,
            LineNumber = lineNumber.Value,
            Side = line.Kind == DiffLineKind.Deleted
                ? ReviewCommentSide.Left
                : SelectedDiffLine.CommentSide,
            Body = body,
            SourceLineText = line.Text,
            SelectionStart = SelectedDiffLine.SelectionLength > 0
                ? SelectedDiffLine.SelectionStart
                : null,
            SelectionLength = SelectedDiffLine.SelectionLength,
            SelectedText = SelectedCodeFragment,
            SuggestedReplacement = suggestion,
            IterationId = SelectedIteration?.IsAllChanges == false
                ? SelectedIteration.Id
                : null,
            CommitSha = ReviewSessionService.GetActiveRange(state).HeadCommitSha
        };
        state.Comments.Add(comment);
        DraftComment = string.Empty;
        DraftSuggestion = string.Empty;
        SelectedCodeFragment = string.Empty;
        RefreshComments();
        if (await SaveStateAsync())
        {
            StatusMessage = "Comment queued locally.";
        }
    }

    [RelayCommand]
    private async Task PublishCommentsAsync()
    {
        if (state is null)
        {
            return;
        }

        IsBusy = true;
        ErrorMessage = string.Empty;
        try
        {
            CommentPublishResult result = await sessionService.PublishPendingCommentsAsync(state);
            RefreshComments();
            StatusMessage = result.Failed == 0
                ? $"Published {result.Published} comment(s)."
                : $"Published {result.Published} comment(s); {result.Failed} failed. Review the errors below.";
        }
        catch (ReviewAuthenticationException exception)
        {
            ErrorMessage = string.Empty;
            StatusMessage = exception.Message;
            RefreshComments();
            AuthenticationRequired?.Invoke(
                exception.Reference,
                PublishCommentsAsync);
        }
        catch (HttpRequestException exception)
        {
            ErrorMessage = exception.Message;
        }
        catch (IOException exception)
        {
            ErrorMessage = exception.Message;
        }
        catch (ReviewOperationException exception)
        {
            ErrorMessage = exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    [RelayCommand]
    private Task RefreshAsync() => RefreshFullAsync();

    public async Task RefreshFullAsync()
    {
        if (state is null)
        {
            return;
        }

        await RunBusyAsync(async () =>
        {
            state = await sessionService.RefreshRepositoryAsync(state);
            RebuildDescriptionMarkdown();
            RebuildIterations();
            RebuildFiles();
            RefreshComments();
            FileTreeNodeViewModel? selectedNode = string.IsNullOrWhiteSpace(
                selectedPath)
                ? null
                : FindFileNode(FileRoots, selectedPath);
            SelectedTreeNode = selectedNode;
            if (selectedNode is not null)
            {
                selectedNode.ExpandAncestors();
            }
            else
            {
                selectedPath = string.Empty;
                selectedPathIsFolder = false;
                OnPropertyChanged(nameof(IsFolderSelection));
            }

            await ReloadDiffCoreAsync();
            StatusMessage = "Pull request refreshed.";
            NotifyIdentityChanged();
            ReviewStateChanged?.Invoke(this, EventArgs.Empty);
        });
    }

    [RelayCommand(CanExecute = nameof(CanMarkReviewed))]
    private async Task MarkReviewedAsync()
    {
        if (state is null)
        {
            return;
        }

        await RunBusyAsync(async () =>
        {
            await sessionService.MarkIterationReviewedAsync(state);
            StatusMessage = "Latest pull request iteration marked reviewed.";
            NotifyIdentityChanged();
            ReviewStateChanged?.Invoke(this, EventArgs.Empty);
        });
    }

    [RelayCommand]
    private void NextChange()
    {
        NavigateChange(1);
    }

    [RelayCommand]
    private void PreviousChange()
    {
        NavigateChange(-1);
    }

    private void NavigateChange(int direction)
    {
        DiffLineViewModel[] changes = DiffLines.Where(line => line.IsChange).ToArray();
        if (changes.Length == 0)
        {
            StatusMessage = "This view has no changed lines.";
            return;
        }

        int currentIndex = SelectedDiffLine is null
            ? -1
            : Array.FindIndex(
                changes,
                change => ReferenceEquals(change.Line, SelectedDiffLine.Line));
        int targetIndex = direction > 0
            ? (currentIndex + 1) % changes.Length
            : (currentIndex <= 0 ? changes.Length - 1 : currentIndex - 1);
        DiffLineViewModel target = changes[targetIndex];
        SelectedDiffLine = target;
        OnPropertyChanged(nameof(ChangePosition));
        OnPropertyChanged(nameof(WorkspaceStatus));
        RequestScrollToLine(target);
        StatusMessage = $"Showing change {targetIndex + 1} of {changes.Length}.";
    }

    private bool CanQueueComment() =>
        ReviewActionState.CanSaveComment(
            state is not null,
            SelectedDiffLine?.IsCommentable == true &&
            !isViewingDefinition,
            DraftComment,
            IsUnchangedSuggestion ? string.Empty : DraftSuggestion,
            IsBusy);

    private bool IsUnchangedSuggestion =>
        string.IsNullOrWhiteSpace(DraftComment) &&
        DraftSuggestion.Equals(focusedSuggestionSeed, StringComparison.Ordinal);

    private string GetFocusedCodeContext()
    {
        if (SelectedDiffLine is null)
        {
            return string.Empty;
        }

        int index = DiffLines.IndexOf(SelectedDiffLine);
        if (index < 0)
        {
            return SelectedDiffLine.CodeText;
        }

        int start = Math.Max(0, index - 1);
        int end = Math.Min(DiffLines.Count - 1, index + 1);
        return string.Join(
            Environment.NewLine,
            DiffLines
                .Skip(start)
                .Take(end - start + 1)
                .Select(line => line.CodeText));
    }

    public void PrepareFocusedComment()
    {
        if (string.IsNullOrWhiteSpace(DraftSuggestion))
        {
            focusedSuggestionSeed = string.IsNullOrWhiteSpace(SelectedCodeFragment)
                ? FocusedCodePreview
                : SelectedCodeFragment;
            DraftSuggestion = focusedSuggestionSeed;
        }
    }

    private bool CanPublishComments() =>
        ReviewActionState.CanPublishComments(
            PendingCommentCount,
            IsBusy);

    private async Task ReloadDiffAsync()
    {
        await RunBusyAsync(async () =>
        {
            await ReloadDiffCoreAsync();
        });
    }

    private async Task ReloadDiffCoreAsync()
    {
        if (state is null || string.IsNullOrWhiteSpace(selectedPath))
        {
            DiffLines.Clear();
            VisibleDiffLines.Clear();
            DiffRows.Clear();
            DiffMarkers.Clear();
            functionRegions = [];
            isViewingDefinition = false;
            OnPropertyChanged(nameof(IsViewingDefinition));
            collapsedFunctionStarts.Clear();
            ClearSearch();
            MarkdownBlocks.Clear();
            selectedMarkdownFile = false;
            NotifyMarkdownPresentationChanged();
            NotifyPresentationChanged();
            return;
        }

        loadCancellation?.Cancel();
        loadCancellation?.Dispose();
        loadCancellation = new CancellationTokenSource();
        CancellationToken cancellationToken = loadCancellation.Token;
        IReadOnlyList<DiffBlock> blocks = await sessionService.LoadDiffAsync(
            state,
            selectedPath,
            ContextLines,
            !selectedPathIsFolder,
            cancellationToken);

        DiffLines.Clear();
        foreach (DiffBlock block in blocks)
        {
            DiffLines.Add(CreateLineViewModel(new DiffLine
            {
                Kind = DiffLineKind.Hunk,
                FilePath = block.FilePath,
                Text = block.PreviousPath is null
                    ? block.FilePath
                    : $"{block.PreviousPath} -> {block.FilePath}"
            }));
            foreach (DiffLine line in block.Lines)
            {
                DiffLines.Add(CreateLineViewModel(line));
            }
        }

        ApplyInlineDifferences();
        collapsedFunctionStarts.Clear();
        functionRegions = selectedPathIsFolder
            ? []
            : CodeFoldBuilder.Build(DiffLines.Select(line => line.Line).ToArray());
        ApplyFoldConfiguration();
        DiffHeading = selectedPathIsFolder
            ? $"{selectedPath} - changed hunks"
            : $"{selectedPath} - complete file diff";
        DiffLineViewModel? firstChange = DiffLines.FirstOrDefault(line => line.IsChange);
        SelectedDiffLine = firstChange;
        OnPropertyChanged(nameof(ChangeCount));
        OnPropertyChanged(nameof(ChangePosition));
        RebuildVisibleDiffLines();
        RebuildDiffRows();
        RebuildDiffMarkers();
        UpdateSearchResults();
        await RebuildMarkdownAsync();
        if (firstChange is not null)
        {
            RequestScrollToLine(firstChange);
        }
    }

    private DiffLineViewModel CreateLineViewModel(
        DiffLine line,
        ReviewCommentSide? displaySide = null)
    {
        var lineViewModel = new DiffLineViewModel(
            line,
            NavigateToDefinitionAsync,
            SelectCodeRange,
            GetInlineComments(line, displaySide),
            displaySide,
            ToggleFunctionFold,
            RequestFocusedComment,
            allowCommenting: !isViewingDefinition);
        lineViewModel.SetFocusedCommentMode(isFocusedCommentMode);
        lineViewModel.SetSelected(
            SelectedDiffLine is not null &&
            ReferenceEquals(line, SelectedDiffLine.Line));
        DiffLineViewModel? source = DiffLines.FirstOrDefault(
            item => ReferenceEquals(item.Line, line));
        if (source is not null && displaySide is not null)
        {
            lineViewModel.SetSearchState(
                source.IsSearchMatch,
                source.IsCurrentSearchMatch,
                SearchQuery);
            lineViewModel.CopyDifferenceFrom(source);
        }

        return lineViewModel;
    }

    private void RequestFocusedComment(DiffLineViewModel line)
    {
        if (isViewingDefinition)
        {
            return;
        }

        SelectLine(line);
        StatusMessage = "Opening comment composer for the selected line.";
        CommentComposerRequested?.Invoke(line);
    }

    private async Task NavigateToDefinitionAsync(
        DiffLineViewModel line,
        SymbolCandidate candidate)
    {
        if (state is null)
        {
            return;
        }

        await RunBusyAsync(async () =>
        {
            SymbolLocation? location = await sessionService.FindDefinitionAsync(
                state,
                candidate.Name,
                line.FilePath,
                line.CommentSide == ReviewCommentSide.Left
                    ? ReviewSessionService.GetActiveRange(state).BaseCommitSha
                    : ReviewSessionService.GetActiveRange(state).HeadCommitSha,
                line.Line.NewLineNumber ?? line.Line.OldLineNumber);
            if (location is null)
            {
                StatusMessage = $"No repository definition found for {candidate.DisplayName}.";
                return;
            }

            string content = await sessionService.ReadFileAsync(
                state,
                location.FilePath,
                line.CommentSide == ReviewCommentSide.Left
                    ? ReviewSessionService.GetActiveRange(state).BaseCommitSha
                    : ReviewSessionService.GetActiveRange(state).HeadCommitSha);
            string[] lines = content.Replace("\r\n", "\n", StringComparison.Ordinal)
                .Split('\n', StringSplitOptions.None);
            int start = Math.Max(0, location.LineNumber - 1 - 100);
            int end = Math.Min(lines.Length, location.LineNumber - 1 + 201);

            isViewingDefinition = true;
            OnPropertyChanged(nameof(IsViewingDefinition));
            DiffLines.Clear();
            DiffLines.Add(CreateLineViewModel(new DiffLine
            {
                Kind = DiffLineKind.Hunk,
                FilePath = location.FilePath,
                Text = $"Definition: {location.FilePath}:{location.LineNumber}"
            }));
            for (int index = start; index < end; index++)
            {
                DiffLines.Add(CreateLineViewModel(new DiffLine
                {
                    Kind = DiffLineKind.Context,
                    FilePath = location.FilePath,
                    NewLineNumber = index + 1,
                    Text = lines[index]
                }));
            }

            collapsedFunctionStarts.Clear();
            functionRegions = [];
            DiffHeading = $"{location.FilePath}:{location.LineNumber} - {candidate.DisplayName}";
            SelectedDiffLine = DiffLines.FirstOrDefault(
                candidate => candidate.Line.NewLineNumber == location.LineNumber);
            RebuildVisibleDiffLines();
            RebuildDiffRows();
            if (SelectedDiffLine is not null)
            {
                RequestScrollToLine(SelectedDiffLine);
            }
            StatusMessage = $"Navigated to {candidate.DisplayName}.";
        });
    }

    private async Task<bool> RunBusyAsync(Func<Task> action)
    {
        IsBusy = true;
        ErrorMessage = string.Empty;
        try
        {
            await action();
            return true;
        }
        catch (OperationCanceledException)
        {
            return false;
        }
        catch (ReviewOperationException exception)
        {
            ErrorMessage = exception.Message;
            return false;
        }
        catch (IOException exception)
        {
            ErrorMessage = exception.Message;
            return false;
        }
        catch (HttpRequestException exception)
        {
            ErrorMessage = exception.Message;
            return false;
        }
        finally
        {
            IsBusy = false;
        }
    }

    private async Task<bool> SaveStateAsync()
    {
        if (state is null)
        {
            return false;
        }

        return await RunBusyAsync(() => sessionService.SaveAsync(state));
    }

    private void RebuildFiles()
    {
        FileRoots.Clear();
        IEnumerable<ChangedFile> files = state!.ChangedFiles;
        string filter = FileFilter.Trim();
        if (!string.IsNullOrWhiteSpace(filter))
        {
            files = files.Where(file =>
                file.Path.Contains(
                    filter,
                    StringComparison.OrdinalIgnoreCase));
        }

        foreach (FileTreeNodeViewModel node in FileTreeBuilder.Build(
                     files,
                     state.ViewedFiles))
        {
            FileRoots.Add(node);
        }

        SelectedTreeNode = string.IsNullOrWhiteSpace(selectedPath)
            ? null
            : FindFileNode(FileRoots, selectedPath);
        OnPropertyChanged(nameof(ChangedFileCount));
        OnPropertyChanged(nameof(ChangedFilesSummary));
        OnPropertyChanged(nameof(WorkspaceSubtitle));
    }

    private void RebuildIterations()
    {
        if (state is null)
        {
            Iterations.Clear();
            SelectedIteration = null;
            return;
        }

        Iterations.Clear();
        Iterations.Add(new PullRequestIteration
        {
            Id = PullRequestIteration.AllChangesId,
            Number = 0,
            Label = "All changes",
            BaseCommitSha = state.Details.BaseCommitSha,
            HeadCommitSha = state.Details.HeadCommitSha,
            IsAllChanges = true
        });
        foreach (PullRequestIteration iteration in state.Iterations)
        {
            Iterations.Add(iteration);
        }

        SelectedIteration = string.IsNullOrWhiteSpace(state.SelectedIterationId)
            ? Iterations.FirstOrDefault()
            : Iterations.FirstOrDefault(
                iteration => iteration.Id.Equals(
                    state.SelectedIterationId,
                    StringComparison.Ordinal));
        SelectedIteration ??= Iterations.FirstOrDefault();
        OnPropertyChanged(nameof(HasNewIteration));
        OnPropertyChanged(nameof(NewIterationLabel));
        OnPropertyChanged(nameof(LastReviewedLabel));
        OnPropertyChanged(nameof(ReviewStatusLabel));
        OnPropertyChanged(nameof(CanMarkReviewed));
        MarkReviewedCommand.NotifyCanExecuteChanged();
    }

    private void RefreshComments()
    {
        Comments.Clear();
        RemoteComments.Clear();
        if (state is not null)
        {
            foreach (ReviewComment comment in state.Comments.OrderByDescending(comment => comment.CreatedAtUtc))
            {
                Comments.Add(new ReviewCommentViewModel(
                    comment,
                    DeleteReviewCommentAsync,
                    SaveReviewCommentEditAsync));
            }

            HashSet<string> publishedIds = state.Comments
                .Where(comment => !string.IsNullOrWhiteSpace(comment.RemoteId))
                .Select(comment => comment.RemoteId!)
                .ToHashSet(StringComparer.Ordinal);
            IEnumerable<RemoteReviewComment> remoteComments = state.RemoteComments
                .Where(comment =>
                    string.IsNullOrWhiteSpace(comment.RemoteId) ||
                    !publishedIds.Contains(comment.RemoteId))
                .OrderByDescending(comment => comment.CreatedAtUtc);
            foreach (ReviewCommentViewModel thread in BuildRemoteThreads(
                         remoteComments))
            {
                RemoteComments.Add(thread);
            }
        }

        RebuildFilteredComments();
        UpdateInlineComments();
        foreach (MarkdownBlockViewModel block in MarkdownBlocks)
        {
            block.RefreshComments();
        }
        OnPropertyChanged(nameof(PendingCommentCount));
        OnPropertyChanged(nameof(HasPendingComments));
        OnPropertyChanged(nameof(PendingCommentSummary));
        OnPropertyChanged(nameof(PublishButtonLabel));
        OnPropertyChanged(nameof(CanSaveCommentLocally));
        OnPropertyChanged(nameof(CanPublishPendingComments));
        RebuildDiffRows();
        RebuildDiffMarkers();
    }

    private void NotifyIdentityChanged()
    {
        OnPropertyChanged(nameof(NavigationTitle));
        OnPropertyChanged(nameof(NavigationPrimaryTitle));
        OnPropertyChanged(nameof(NavigationSecondaryTitle));
        OnPropertyChanged(nameof(PullRequestNumber));
        OnPropertyChanged(nameof(NavigationTooltip));
        OnPropertyChanged(nameof(Title));
        OnPropertyChanged(nameof(Author));
        OnPropertyChanged(nameof(ProviderLabel));
        OnPropertyChanged(nameof(PullRequestUrl));
        OnPropertyChanged(nameof(PullRequestUri));
        OnPropertyChanged(nameof(RepositoryLocation));
        OnPropertyChanged(nameof(RepositorySourceLabel));
        OnPropertyChanged(nameof(HasActiveReview));
        OnPropertyChanged(nameof(HasError));
        OnPropertyChanged(nameof(PendingCommentCount));
        OnPropertyChanged(nameof(HasPendingComments));
        OnPropertyChanged(nameof(PendingCommentSummary));
        OnPropertyChanged(nameof(PublishButtonLabel));
        OnPropertyChanged(nameof(IsOpen));
        OnPropertyChanged(nameof(IsPastReview));
        OnPropertyChanged(nameof(HasNewIteration));
        OnPropertyChanged(nameof(NewIterationLabel));
        OnPropertyChanged(nameof(ReviewStatusLabel));
        OnPropertyChanged(nameof(LastReviewedLabel));
        OnPropertyChanged(nameof(HasDescription));
        OnPropertyChanged(nameof(ChangedFileCount));
        OnPropertyChanged(nameof(ChangedFilesSummary));
    }

    partial void OnErrorMessageChanged(string value) =>
        OnPropertyChanged(nameof(HasError));

    partial void OnInspectorTabIndexChanged(int value)
    {
        NotifyInspectorTabsChanged();
    }

    private void NotifyInspectorTabsChanged()
    {
        OnPropertyChanged(nameof(IsCommentsInspectorVisible));
        OnPropertyChanged(nameof(IsOverviewInspectorVisible));
        OnPropertyChanged(nameof(IsNotesInspectorVisible));
    }

    partial void OnNotesChanged(string value)
    {
        if (state is null || suppressNotesChanges)
        {
            return;
        }

        _ = SaveNotesAfterDebounceAsync();
    }

    partial void OnShowOnlyCurrentFileCommentsChanged(bool value) =>
        RebuildFilteredComments();

    partial void OnHideBotCommentsChanged(bool value) =>
        RebuildFilteredComments();

    partial void OnDraftCommentChanged(string value) =>
        OnPropertyChanged(nameof(CanSaveCommentLocally));

    partial void OnDraftSuggestionChanged(string value) =>
        OnPropertyChanged(nameof(CanSaveCommentLocally));

    partial void OnSelectedCodeFragmentChanged(string value)
    {
        OnPropertyChanged(nameof(HasSelectedCodeFragment));
        OnPropertyChanged(nameof(SelectedLineDescription));
    }

    partial void OnSelectedDiffLineChanged(DiffLineViewModel? value)
    {
        foreach (DiffLineViewModel line in DiffLines)
        {
            line.SetSelected(
                value is not null &&
                ReferenceEquals(line.Line, value.Line));
        }

        SelectedCodeFragment = string.Empty;
        DraftSuggestion = string.Empty;
        focusedSuggestionSeed = string.Empty;
        if (IsSideBySideDiff)
        {
            RebuildDiffRows();
        }

        OnPropertyChanged(nameof(SelectedLineDescription));
        OnPropertyChanged(nameof(ChangePosition));
        OnPropertyChanged(nameof(WorkspaceStatus));
        OnPropertyChanged(nameof(CanSaveCommentLocally));
        OnPropertyChanged(nameof(FocusedCodePreview));
        OnPropertyChanged(nameof(HasFocusedCodePreview));
        OnPropertyChanged(nameof(FocusedCodeContext));
    }

    partial void OnDiffLayoutModeIndexChanged(int value)
    {
        OnPropertyChanged(nameof(IsInlineDiff));
        OnPropertyChanged(nameof(IsSideBySideDiff));
        OnPropertyChanged(nameof(IsInlineRawCodeVisible));
        OnPropertyChanged(nameof(IsSideBySideRawCodeVisible));
        RebuildDiffRows();
        if (SelectedDiffLine is not null)
        {
            RequestScrollToLine(SelectedDiffLine);
        }
    }

    partial void OnIsBusyChanged(bool value)
    {
        OnPropertyChanged(nameof(CanPublishPendingComments));
        OnPropertyChanged(nameof(CanSaveCommentLocally));
    }

    private void SelectCodeRange(
        DiffLineViewModel line,
        int start,
        int length)
    {
        if (!ReferenceEquals(SelectedDiffLine, line))
        {
            SelectedDiffLine = line;
        }

        SelectedCodeFragment = length > 0
            ? line.CodeText.Substring(start, length)
            : string.Empty;
        OnPropertyChanged(nameof(SelectedLineDescription));
        OnPropertyChanged(nameof(CanSaveCommentLocally));
    }

    private IReadOnlyList<ReviewCommentViewModel> GetInlineComments(
        DiffLine line,
        ReviewCommentSide? displaySide = null)
    {
        if (state is null)
        {
            return [];
        }

        var comments = state.Comments
            .Where(comment =>
                (displaySide is null || comment.Side == displaySide) &&
                CommentMatchesLine(comment.FilePath, comment.LineNumber, comment.Side, line))
            .Select(comment => new ReviewCommentViewModel(
                comment,
                DeleteReviewCommentAsync,
                SaveReviewCommentEditAsync))
            .ToList();
        HashSet<string> publishedIds = state.Comments
            .Where(comment => !string.IsNullOrWhiteSpace(comment.RemoteId))
            .Select(comment => comment.RemoteId!)
            .ToHashSet(StringComparer.Ordinal);
        IEnumerable<RemoteReviewComment> matchingRemoteComments = state.RemoteComments
            .Where(comment =>
                !publishedIds.Contains(comment.RemoteId) &&
                (displaySide is null || comment.Side == displaySide) &&
                comment.LineNumber is int lineNumber &&
                comment.Side is ReviewCommentSide side &&
                CommentMatchesLine(comment.FilePath, lineNumber, side, line));
        comments.AddRange(BuildRemoteThreads(
            matchingRemoteComments,
            showJumpToLine: false));

        return comments
            .OrderBy(comment => comment.Id, StringComparer.Ordinal)
            .ToArray();
    }

    private async Task DeleteReviewCommentAsync(ReviewComment comment)
    {
        if (state is null)
        {
            return;
        }

        await RunBusyAsync(async () =>
        {
            try
            {
                await sessionService.DeleteCommentAsync(state, comment);
                RefreshComments();
                StatusMessage = comment.Status == ReviewCommentStatus.Published
                    ? "Published comment deleted."
                    : "Saved comment deleted.";
            }
            catch (ReviewAuthenticationException exception)
            {
                ErrorMessage = string.Empty;
                AuthenticationRequired?.Invoke(
                    exception.Reference,
                    () => DeleteReviewCommentAsync(comment));
            }
        });
    }

    private async Task<bool> SaveReviewCommentEditAsync(
        ReviewComment comment,
        string body,
        string suggestion)
    {
        if (state is null ||
            !state.Comments.Contains(comment))
        {
            return false;
        }

        string previousBody = comment.Body;
        string previousSuggestion = comment.SuggestedReplacement;
        comment.Body = body;
        comment.SuggestedReplacement = suggestion;
        comment.Error = null;
        if (!await SaveStateAsync())
        {
            comment.Body = previousBody;
            comment.SuggestedReplacement = previousSuggestion;
            return false;
        }

        RefreshComments();
        StatusMessage = "Saved comment draft updated.";
        return true;
    }

    private async Task ReplyToRemoteCommentAsync(
        RemoteReviewComment comment,
        string body)
    {
        if (state is null)
        {
            return;
        }

        await RunBusyAsync(async () =>
        {
            try
            {
                await sessionService.ReplyToCommentAsync(
                    state,
                    comment,
                    body);
                RefreshComments();
                StatusMessage = "Reply posted.";
            }
            catch (ReviewAuthenticationException exception)
            {
                ErrorMessage = string.Empty;
                AuthenticationRequired?.Invoke(
                    exception.Reference,
                    () => ReplyToRemoteCommentAsync(comment, body));
            }
        });
    }

    private IReadOnlyList<ReviewCommentViewModel> BuildRemoteThreads(
        IEnumerable<RemoteReviewComment> comments,
        bool showJumpToLine = true)
    {
        return comments
            .GroupBy(comment => string.IsNullOrWhiteSpace(comment.ThreadId)
                ? comment.RemoteId
                : comment.ThreadId)
            .Select(group =>
            {
                RemoteReviewComment[] ordered = group
                    .OrderBy(comment => comment.CreatedAtUtc)
                    .ToArray();
                RemoteReviewComment root = ordered.FirstOrDefault(
                    comment => string.IsNullOrWhiteSpace(comment.ParentRemoteId)) ??
                    ordered[0];
                var rootViewModel = new ReviewCommentViewModel(
                    root,
                    NavigateToRemoteCommentAsync,
                    ReplyToRemoteCommentAsync,
                    showJumpToLine);
                rootViewModel.SetReplies(
                    ordered
                        .Where(comment => !ReferenceEquals(comment, root))
                        .OrderBy(comment => comment.CreatedAtUtc)
                        .Select(comment => new ReviewCommentViewModel(
                            comment,
                            NavigateToRemoteCommentAsync,
                            showJumpToLine: showJumpToLine)));
                return rootViewModel;
            })
            .ToArray();
    }

    private static bool CommentMatchesLine(
        string filePath,
        int lineNumber,
        ReviewCommentSide side,
        DiffLine line)
    {
        if (!filePath.Replace('\\', '/').Equals(
                line.FilePath.Replace('\\', '/'),
                StringComparison.OrdinalIgnoreCase))
        {
            return false;
        }

        int? lineForSide = side == ReviewCommentSide.Left
            ? line.OldLineNumber
            : line.NewLineNumber;
        return lineForSide == lineNumber;
    }

    private void UpdateInlineComments()
    {
        foreach (DiffLineViewModel line in DiffLines)
        {
            line.SetComments(GetInlineComments(line.Line));
        }
    }

    private void ToggleFunctionFold(DiffLineViewModel line)
    {
        CodeFoldRegion? region = line.FoldRegion;
        if (region is null)
        {
            return;
        }

        if (!collapsedFunctionStarts.Add(region.StartIndex))
        {
            collapsedFunctionStarts.Remove(region.StartIndex);
        }

        ApplyFoldConfiguration();
        RebuildVisibleDiffLines();
        RebuildDiffRows();
        RebuildDiffMarkers();
    }

    private void ApplyFoldConfiguration()
    {
        foreach (DiffLineViewModel line in DiffLines)
        {
            line.ConfigureFold(null, false);
        }

        foreach (CodeFoldRegion region in functionRegions)
        {
            if (region.StartIndex >= 0 && region.StartIndex < DiffLines.Count)
            {
                DiffLines[region.StartIndex].ConfigureFold(
                    region,
                    collapsedFunctionStarts.Contains(region.StartIndex));
            }
        }

        OnPropertyChanged(nameof(HasFunctionFolds));
    }

    private void RebuildVisibleDiffLines()
    {
        VisibleDiffLines.Clear();
        for (int index = 0; index < DiffLines.Count; index++)
        {
            CodeFoldRegion? collapsedRegion = functionRegions.FirstOrDefault(
                region =>
                    region.StartIndex == index &&
                    collapsedFunctionStarts.Contains(region.StartIndex));
            VisibleDiffLines.Add(DiffLines[index]);
            if (collapsedRegion is not null)
            {
                index = Math.Min(collapsedRegion.EndIndex, DiffLines.Count - 1);
            }
        }
    }

    private void RebuildDiffMarkers()
    {
        DiffMarkers.Clear();
        ChangedFile? selectedFile = state?.ChangedFiles.FirstOrDefault(
            file => file.Path.Equals(
                selectedPath,
                StringComparison.OrdinalIgnoreCase));
        if (selectedPathIsFolder ||
            selectedFile is null ||
            selectedFile.Status is not (
                ChangedFileStatus.Modified or
                ChangedFileStatus.Renamed or
                ChangedFileStatus.Copied or
                ChangedFileStatus.TypeChanged))
        {
            NotifyPresentationChanged();
            DiffMarkersChanged?.Invoke(this, EventArgs.Empty);
            return;
        }

        int visibleLineCount = Math.Max(1, VisibleDiffLines.Count);
        for (int index = 0; index < DiffLines.Count; index++)
        {
            DiffLineViewModel line = DiffLines[index];
            if (!line.IsChange ||
                index > 0 && DiffLines[index - 1].IsChange)
            {
                continue;
            }

            int lineNumber = line.Line.NewLineNumber ??
                line.Line.OldLineNumber ??
                1;
            int visibleIndex = VisibleDiffLines.IndexOf(line);
            DiffMarkers.Add(new DiffChangeMarkerViewModel(
                visibleIndex <= 0
                    ? 0
                    : visibleIndex / (double)visibleLineCount,
                $"{line.FilePath}:{lineNumber}",
                line,
                NavigateToDiffLine,
                line.IsAdded));
        }

        foreach (DiffLineViewModel line in VisibleDiffLines.Where(
                     line => line.HasComments))
        {
            int visibleIndex = VisibleDiffLines.IndexOf(line);
            int lineNumber = line.Line.NewLineNumber ??
                line.Line.OldLineNumber ??
                1;
            DiffMarkers.Add(new DiffChangeMarkerViewModel(
                visibleIndex <= 0
                    ? 0
                    : visibleIndex / (double)visibleLineCount,
                $"Comment at {line.FilePath}:{lineNumber}",
                line,
                NavigateToDiffLine,
                isAddition: false,
                isComment: true));
        }

        NotifyPresentationChanged();
        DiffMarkersChanged?.Invoke(this, EventArgs.Empty);
    }

    private void NavigateToDiffLine(DiffLineViewModel line)
    {
        EnsureLineVisible(line);
        SelectedDiffLine = line;
        RequestScrollToLine(line);
    }

    private void EnsureLineVisible(DiffLineViewModel line)
    {
        int index = DiffLines.IndexOf(line);
        if (index < 0)
        {
            return;
        }

        foreach (CodeFoldRegion region in functionRegions)
        {
            if (index > region.StartIndex &&
                index <= region.EndIndex)
            {
                collapsedFunctionStarts.Remove(region.StartIndex);
            }
        }

        ApplyFoldConfiguration();
        RebuildVisibleDiffLines();
        RebuildDiffRows();
    }

    partial void OnSearchQueryChanged(string value) =>
        QueueSearchUpdate(value);

    partial void OnFileFilterChanged(string value)
    {
        if (state is not null)
        {
            RebuildFiles();
        }
    }

    private void UpdateSearchResults()
    {
        foreach (DiffLineViewModel line in DiffLines)
        {
            line.SetSearchState(false, false);
        }

        string query = CanSearchFile ? SearchQuery.Trim() : string.Empty;
        searchMatches = string.IsNullOrWhiteSpace(query)
            ? []
            : DiffLines
                .Where(line =>
                    line.Line.Kind != DiffLineKind.Hunk &&
                    line.CodeText.Contains(
                        query,
                        StringComparison.OrdinalIgnoreCase))
                .ToList();
        foreach (DiffLineViewModel match in searchMatches)
        {
            match.SetSearchState(true, false, query);
        }

        currentSearchIndex = -1;
        if (searchMatches.Count > 0)
        {
            NavigateSearch(1);
        }
        else
        {
            RebuildDiffRows();
            NotifySearchChanged();
        }
    }

    private void NavigateSearch(int direction)
    {
        if (searchMatches.Count == 0)
        {
            return;
        }

        if (currentSearchIndex >= 0 &&
            currentSearchIndex < searchMatches.Count)
        {
            searchMatches[currentSearchIndex].SetSearchState(
                true,
                false,
                SearchQuery);
        }

        currentSearchIndex = currentSearchIndex < 0
            ? direction > 0 ? 0 : searchMatches.Count - 1
            : (currentSearchIndex + direction + searchMatches.Count) %
              searchMatches.Count;
        DiffLineViewModel target = searchMatches[currentSearchIndex];
        target.SetSearchState(true, true, SearchQuery);
        EnsureLineVisible(target);
        SelectedDiffLine = target;
        RequestScrollToLine(target);
        RebuildDiffRows();
        NotifySearchChanged();
    }

    private void ClearSearch()
    {
        searchMatches = [];
        currentSearchIndex = -1;
        OnPropertyChanged(nameof(SearchQuery));
        NotifySearchChanged();
    }

    private CancellationTokenSource? searchDebounce;

    private async void QueueSearchUpdate(string value)
    {
        searchDebounce?.Cancel();
        searchDebounce?.Dispose();
        searchDebounce = new CancellationTokenSource();
        CancellationToken token = searchDebounce.Token;
        OnPropertyChanged(nameof(HasSearchQuery));
        try
        {
            await Task.Delay(150, token);
            if (value.Equals(SearchQuery, StringComparison.Ordinal))
            {
                UpdateSearchResults();
            }
        }
        catch (OperationCanceledException)
        {
        }
    }

    private void NotifySearchChanged()
    {
        OnPropertyChanged(nameof(SearchMatchCount));
        OnPropertyChanged(nameof(SearchPosition));
        OnPropertyChanged(nameof(HasSearchMatches));
        OnPropertyChanged(nameof(CanMoveSearchPrevious));
        OnPropertyChanged(nameof(CanMoveSearchNext));
        OnPropertyChanged(nameof(CanSearchFile));
        OnPropertyChanged(nameof(HasSearchQuery));
        PreviousSearchResultCommand.NotifyCanExecuteChanged();
        NextSearchResultCommand.NotifyCanExecuteChanged();
    }

    private async Task SaveNotesAfterDebounceAsync()
    {
        notesSaveDebounce?.Cancel();
        notesSaveDebounce?.Dispose();
        notesSaveDebounce = new CancellationTokenSource();
        CancellationToken token = notesSaveDebounce.Token;
        NotesSaveStatus = "Saving…";
        try
        {
            await Task.Delay(500, token);
            state!.Notes = Notes;
            await sessionService.SaveAsync(state, token);
            NotesSaveStatus = "Saved on this device.";
        }
        catch (OperationCanceledException)
        {
        }
        catch (IOException exception)
        {
            ErrorMessage = exception.Message;
            NotesSaveStatus = "Could not save notes.";
        }
        catch (ReviewOperationException exception)
        {
            ErrorMessage = exception.Message;
            NotesSaveStatus = "Could not save notes.";
        }
    }

    private void RebuildFilteredComments()
    {
        VisibleComments.Clear();
        VisibleRemoteComments.Clear();
        foreach (ReviewCommentViewModel comment in Comments.Where(CommentIsVisible))
        {
            VisibleComments.Add(comment);
        }

        foreach (ReviewCommentViewModel comment in RemoteComments.Where(CommentIsVisible))
        {
            VisibleRemoteComments.Add(comment);
        }
    }

    private bool CommentIsVisible(ReviewCommentViewModel comment) =>
        (!HideBotComments || !comment.IsBot) &&
        (!ShowOnlyCurrentFileComments ||
         comment.MatchesFile(selectedPath));

    private void NotifyPresentationChanged()
    {
        OnPropertyChanged(nameof(HasFunctionFolds));
        OnPropertyChanged(nameof(HasDiffMarkers));
        OnPropertyChanged(nameof(AreDiffMarkersVisible));
        OnPropertyChanged(nameof(CanSearchFile));
    }

    private string GetChangePosition()
    {
        if (string.IsNullOrWhiteSpace(selectedPath))
        {
            return "Select a file to view changes";
        }

        DiffLineViewModel[] changes = DiffLines.Where(line => line.IsChange).ToArray();
        if (changes.Length == 0)
        {
            return "No changes";
        }

        int index = SelectedDiffLine is null
            ? -1
            : Array.FindIndex(
                changes,
                change => ReferenceEquals(change.Line, SelectedDiffLine.Line));
        return index < 0
            ? $"{changes.Length} changed lines"
            : $"{index + 1} of {changes.Length} changes";
    }

    private void RequestScrollToLine(DiffLineViewModel line)
    {
        ScrollToLineRequested?.Invoke(line);
    }

    public DiffRowViewModel? FindDiffRow(DiffLineViewModel line) =>
        DiffRows.FirstOrDefault(row => row.Contains(line));

    private async Task NavigateToRemoteCommentAsync(
        RemoteReviewComment comment)
    {
        if (state is null ||
            string.IsNullOrWhiteSpace(comment.FilePath) ||
            comment.LineNumber is not int lineNumber ||
            comment.Side is not ReviewCommentSide side)
        {
            return;
        }

        await RunBusyAsync(async () =>
        {
            string normalizedPath = comment.FilePath.Replace('\\', '/');
            FileTreeNodeViewModel? fileNode = FindFileNode(
                FileRoots,
                normalizedPath);
            selectedPath = normalizedPath;
            selectedPathIsFolder = false;
            OnPropertyChanged(nameof(IsFolderSelection));
            SelectedTreeNode = fileNode;

            if (fileNode is not null)
            {
                fileNode.ExpandAncestors();
                await ReloadDiffCoreAsync();
            }
            else
            {
                await ShowFileNearCommentAsync(
                    comment.FilePath,
                    lineNumber,
                    side);
            }

            DiffLineViewModel? target = DiffLines.FirstOrDefault(line =>
                line.FilePath.Replace('\\', '/').Equals(
                    normalizedPath,
                    StringComparison.OrdinalIgnoreCase) &&
                (side == ReviewCommentSide.Left
                    ? line.Line.OldLineNumber
                    : line.Line.NewLineNumber) == lineNumber);
            if (target is null)
            {
                await ShowFileNearCommentAsync(
                    comment.FilePath,
                    lineNumber,
                    side);
                target = DiffLines.FirstOrDefault(line =>
                    (side == ReviewCommentSide.Left
                        ? line.Line.OldLineNumber
                        : line.Line.NewLineNumber) == lineNumber);
            }

            if (target is null)
            {
                StatusMessage =
                    $"The comment location {comment.FilePath}:{lineNumber} is not present in this revision.";
                return;
            }

            SelectedDiffLine = target;
            RequestScrollToLine(target);
            StatusMessage = $"Navigated to {comment.FilePath}:{lineNumber}.";
        });
    }

    private async Task ShowFileNearCommentAsync(
        string filePath,
        int lineNumber,
        ReviewCommentSide side)
    {
        if (state is null)
        {
            return;
        }

        PullRequestReviewRange range = ReviewSessionService.GetActiveRange(state);
        string commit = side == ReviewCommentSide.Left
            ? range.BaseCommitSha
            : range.HeadCommitSha;
        string content = await sessionService.ReadFileAsync(
            state,
            filePath,
            commit);
        string[] lines = content.Replace("\r\n", "\n", StringComparison.Ordinal)
            .Split('\n', StringSplitOptions.None);
        int targetIndex = Math.Clamp(lineNumber - 1, 0, Math.Max(lines.Length - 1, 0));
        int start = Math.Max(0, targetIndex - 50);
        int end = Math.Min(lines.Length, targetIndex + 51);

        DiffLines.Clear();
        DiffLines.Add(CreateLineViewModel(new DiffLine
        {
            Kind = DiffLineKind.Hunk,
            FilePath = filePath,
            Text = $"Comment: {filePath}:{lineNumber}"
        }));
        for (int index = start; index < end; index++)
        {
            DiffLines.Add(CreateLineViewModel(new DiffLine
            {
                Kind = DiffLineKind.Context,
                FilePath = filePath,
                OldLineNumber = side == ReviewCommentSide.Left
                    ? index + 1
                    : null,
                NewLineNumber = side == ReviewCommentSide.Right
                    ? index + 1
                    : null,
                Text = lines[index]
            }));
        }

        DiffHeading = $"{filePath}:{lineNumber} - comment";
        RebuildVisibleDiffLines();
        RebuildDiffRows();
        RebuildDiffMarkers();
    }

    private async Task RebuildMarkdownAsync()
    {
        selectedMarkdownFile =
            !selectedPathIsFolder &&
            Path.GetExtension(selectedPath).Equals(
                ".md",
                StringComparison.OrdinalIgnoreCase);
        MarkdownBlocks.Clear();
        if (!selectedMarkdownFile || state is null)
        {
            OnPropertyChanged(nameof(IsMarkdownFile));
            NotifyMarkdownPresentationChanged();
            return;
        }

        string content = await sessionService.ReadFileAsync(
            state,
            selectedPath,
            ReviewSessionService.GetActiveRange(state).HeadCommitSha);
        foreach (MarkdownBlock block in MarkdownParser.Parse(content))
        {
            DiffLineViewModel? sourceLine = DiffLines.FirstOrDefault(
                line => line.Line.NewLineNumber == block.SourceLineNumber) ??
                CreateLineViewModel(new DiffLine
                {
                    Kind = DiffLineKind.Context,
                    FilePath = selectedPath,
                    NewLineNumber = block.SourceLineNumber,
                    Text = block.Text
                });
            MarkdownBlocks.Add(new MarkdownBlockViewModel(
                block,
                sourceLine,
                ComposeMarkdownCommentAsync));
        }

        OnPropertyChanged(nameof(IsMarkdownFile));
        NotifyMarkdownPresentationChanged();
    }

    private void NotifyMarkdownPresentationChanged()
    {
        OnPropertyChanged(nameof(IsFormattedMarkdownVisible));
        OnPropertyChanged(nameof(IsRawCodeVisible));
        OnPropertyChanged(nameof(IsInlineRawCodeVisible));
        OnPropertyChanged(nameof(IsSideBySideRawCodeVisible));
        OnPropertyChanged(nameof(AreDiffMarkersVisible));
        OnPropertyChanged(nameof(MarkdownToggleLabel));
    }

    private void RebuildDescriptionMarkdown()
    {
        DescriptionMarkdownBlocks.Clear();
        foreach (MarkdownBlock block in MarkdownParser.Parse(
                     state?.Details.Description ?? string.Empty))
        {
            DescriptionMarkdownBlocks.Add(new MarkdownBlockViewModel(block));
        }

        OnPropertyChanged(nameof(HasDescription));
    }

    private Task ComposeMarkdownCommentAsync(DiffLineViewModel line)
    {
        SelectedDiffLine = line;
        CommentComposerRequested?.Invoke(line);
        return Task.CompletedTask;
    }

    private static FileTreeNodeViewModel? FindFileNode(
        IEnumerable<FileTreeNodeViewModel> roots,
        string path)
    {
        string normalized = path.Replace('\\', '/');
        foreach (FileTreeNodeViewModel node in roots)
        {
            if (node.Path.Equals(normalized, StringComparison.OrdinalIgnoreCase))
            {
                return node;
            }

            FileTreeNodeViewModel? nested = FindFileNode(node.Children, path);
            if (nested is not null)
            {
                return nested;
            }
        }

        return null;
    }

    private static FileTreeNodeViewModel? FindFirstFile(
        IEnumerable<FileTreeNodeViewModel> roots)
    {
        foreach (FileTreeNodeViewModel node in roots)
        {
            if (node.IsFile)
            {
                return node;
            }

            FileTreeNodeViewModel? nested = FindFirstFile(node.Children);
            if (nested is not null)
            {
                return nested;
            }
        }

        return null;
    }

    private void RebuildDiffRows()
    {
        DiffRows.Clear();
        if (DiffLayoutModeIndex == 0)
        {
            return;
        }

        foreach (DiffLinePair pair in DiffSideBySideBuilder.Build(
                     VisibleDiffLines.Select(line => line.Line).ToArray()))
        {
            if (pair.HunkLine is not null)
            {
                DiffRows.Add(new DiffRowViewModel(
                    CreateLineViewModel(pair.HunkLine)));
                continue;
            }

            DiffRows.Add(new DiffRowViewModel(
                hunk: null,
                left: pair.LeftLine is null
                    ? null
                    : CreateLineViewModel(
                        pair.LeftLine,
                        ReviewCommentSide.Left),
                right: pair.RightLine is null
                    ? null
                    : CreateLineViewModel(
                        pair.RightLine,
                        ReviewCommentSide.Right)));
        }
    }

    private void ApplyInlineDifferences()
    {
        foreach (DiffLinePair pair in DiffSideBySideBuilder.Build(
                     DiffLines.Select(line => line.Line).ToArray()))
        {
            if (pair.LeftLine?.Kind != DiffLineKind.Deleted ||
                pair.RightLine?.Kind != DiffLineKind.Added)
            {
                continue;
            }

            DiffLineViewModel? left = DiffLines.FirstOrDefault(
                line => ReferenceEquals(line.Line, pair.LeftLine));
            DiffLineViewModel? right = DiffLines.FirstOrDefault(
                line => ReferenceEquals(line.Line, pair.RightLine));
            if (left is null || right is null)
            {
                continue;
            }

            left.SetDifference(right.CodeText);
            right.SetDifference(left.CodeText);
        }
    }

    private static bool IsProviderTerminalState(PullRequestDetails details) =>
        details.IsClosed ||
        details.State.Equals("closed", StringComparison.OrdinalIgnoreCase) ||
        details.State.Equals("completed", StringComparison.OrdinalIgnoreCase) ||
        details.State.Equals("abandoned", StringComparison.OrdinalIgnoreCase);
}
