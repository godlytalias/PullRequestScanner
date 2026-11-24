using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using PullRequestScanner.Core.Diff;
using PullRequestScanner.Core.Models;
using PullRequestScanner.Core.Navigation;

namespace PullRequestScanner.App.ViewModels;

public sealed class DiffLineViewModel : ObservableObject
{
    private readonly Action<DiffLineViewModel, int, int> selectCodeRange;
    private readonly Action<DiffLineViewModel>? toggleFold;
    private readonly Action<DiffLineViewModel>? requestFocusedComment;
    private readonly bool allowCommenting;
    private string searchQuery = string.Empty;
    private string differencePrefix = string.Empty;
    private string differenceText = string.Empty;
    private string differenceSuffix = string.Empty;

    public DiffLineViewModel(
        DiffLine line,
        Func<DiffLineViewModel, SymbolCandidate, Task> navigateToDefinition,
        Action<DiffLineViewModel, int, int> selectCodeRange,
        IEnumerable<ReviewCommentViewModel> comments,
        ReviewCommentSide? displaySide = null,
        Action<DiffLineViewModel>? toggleFold = null,
        Action<DiffLineViewModel>? requestFocusedComment = null,
        bool allowCommenting = true)
    {
        Line = line;
        DisplaySide = displaySide;
        this.selectCodeRange = selectCodeRange;
        this.toggleFold = toggleFold;
        this.requestFocusedComment = requestFocusedComment;
        this.allowCommenting = allowCommenting;
        ToggleFoldCommand = new RelayCommand(
            () => toggleFold?.Invoke(this),
            () => FoldRegion is not null);
        OpenFocusedCommentCommand = new RelayCommand(
            () => requestFocusedComment?.Invoke(this),
            () => CanOpenComment && requestFocusedComment is not null);
        NavigationSymbols = SymbolCandidateExtractor.ExtractCandidates(line.Text)
            .Where(candidate => !candidate.IsLikelyExternal)
            .Select(candidate => new SymbolLinkViewModel(
                candidate.DisplayName,
                candidate.ColumnIndex,
                displaySide is null ? 720 : 320,
                new AsyncRelayCommand(() => navigateToDefinition(this, candidate))))
            .ToArray();
        PeekDefinitionCommand = new AsyncRelayCommand(
            async () =>
            {
                if (PrimaryNavigationSymbol is not null)
                {
                    await PrimaryNavigationSymbol.NavigateCommand.ExecuteAsync(null);
                }
            },
            () => PrimaryNavigationSymbol is not null);
        SetComments(comments);
    }

    public DiffLine Line { get; }
    public string OldLineNumber => Line.OldLineNumber?.ToString() ?? string.Empty;
    public string NewLineNumber => Line.NewLineNumber?.ToString() ?? string.Empty;
    public string CodeText => Line.Text;
    public string FilePath => Line.FilePath;
    public ReviewCommentSide? DisplaySide { get; }
    public ReviewCommentSide CommentSide => DisplaySide ??
        (Line.Kind == DiffLineKind.Deleted
            ? ReviewCommentSide.Left
            : ReviewCommentSide.Right);
    public string DisplayLineNumber => DisplaySide switch
    {
        ReviewCommentSide.Left => OldLineNumber,
        ReviewCommentSide.Right => NewLineNumber,
        _ => Line.NewLineNumber?.ToString() ?? OldLineNumber
    };
    public bool ShowOldLineNumber => IsCommentable &&
        DisplaySide != ReviewCommentSide.Right;
    public bool ShowNewLineNumber => IsCommentable &&
        DisplaySide != ReviewCommentSide.Left;
    public string Marker => Line.Kind switch
    {
        DiffLineKind.Added => "+",
        DiffLineKind.Deleted => "-",
        _ => string.Empty
    };
    public bool IsAdded => Line.Kind == DiffLineKind.Added;
    public bool IsDeleted => Line.Kind == DiffLineKind.Deleted;
    public bool IsHunk => Line.Kind == DiffLineKind.Hunk;
    public bool IsContext => Line.Kind == DiffLineKind.Context;
    public bool IsChange => Line.Kind is DiffLineKind.Added or DiffLineKind.Deleted;
    public bool IsCommentable => Line.Kind != DiffLineKind.Hunk &&
        (Line.OldLineNumber.HasValue || Line.NewLineNumber.HasValue);
    public bool CanOpenComment => IsCommentable && allowCommenting;
    public bool CanOpenFocusedComment => IsFocusedCommentMode && CanOpenComment;
    public bool ShowNormalCommentGutter => CanOpenComment &&
        !IsFocusedCommentMode &&
        IsSelected &&
        !HasFold;
    public bool ShowCodeTextBox => false;
    public bool ShowCodeTextBlock => !CanOpenFocusedComment;
    public bool CanNavigate => Line.Kind != DiffLineKind.Hunk &&
        NavigationSymbols.Count > 0;
    public bool ShowPeekDefinition => IsSelected && CanNavigate;
    public SymbolLinkViewModel? PrimaryNavigationSymbol =>
        NavigationSymbols.FirstOrDefault();
    public IReadOnlyList<SymbolLinkViewModel> NavigationSymbols { get; }
    public ObservableCollection<ReviewCommentViewModel> Comments { get; } = [];
    public int CommentCount => Comments.Count;
    public bool HasComments => Comments.Count > 0;
    public int SelectionStart { get; private set; }
    public int SelectionLength { get; private set; }
    public CodeFoldRegion? FoldRegion { get; private set; }
    public bool HasFold => FoldRegion is not null;
    public bool IsFoldCollapsed { get; private set; }
    public string FoldGlyph => IsFoldCollapsed ? "\uE76C" : "\uE70D";
    public string FoldSummary => FoldRegion is null
        ? string.Empty
        : $"{FoldRegion.LineCount} lines collapsed";
    public IRelayCommand ToggleFoldCommand { get; }
    public IRelayCommand OpenFocusedCommentCommand { get; }
    public IAsyncRelayCommand PeekDefinitionCommand { get; }
    public bool IsFocusedCommentMode { get; private set; }
    public bool IsSelected { get; private set; }
    public bool IsSearchMatch { get; private set; }
    public bool IsCurrentSearchMatch { get; private set; }
    public GridLength OldLineNumberColumnWidth =>
        ShowOldLineNumber ? new GridLength(36) : new GridLength(0);
    public GridLength NewLineNumberColumnWidth =>
        ShowNewLineNumber ? new GridLength(36) : new GridLength(0);
    public string HighlightPrefix => IsSearchMatch
        ? GetSearchParts().Prefix
        : string.IsNullOrEmpty(differenceText) &&
          string.IsNullOrEmpty(differenceSuffix)
            ? CodeText
            : differencePrefix;
    public string HighlightText => IsSearchMatch
        ? GetSearchParts().Match
        : differenceText;
    public string HighlightSuffix => IsSearchMatch
        ? GetSearchParts().Suffix
        : differenceSuffix;
    public Brush? HighlightBrush =>
        IsSearchMatch
            ? Application.Current.Resources[
                "AccentTextFillColorPrimaryBrush"] as Brush
            : IsAdded
                ? Application.Current.Resources[
                    "AddedDiffLineStrokeBrush"] as Brush
                : IsDeleted
                    ? Application.Current.Resources[
                        "DeletedDiffLineStrokeBrush"] as Brush
                    : null;
    public IReadOnlyList<DiffCodeTextRun> DisplayRuns =>
        CreateDisplayRuns();

    public void SelectCodeRange(int start, int length)
    {
        SelectionStart = Math.Clamp(start, 0, CodeText.Length);
        SelectionLength = Math.Clamp(length, 0, CodeText.Length - SelectionStart);
        selectCodeRange(this, SelectionStart, SelectionLength);
    }

    public void SetComments(IEnumerable<ReviewCommentViewModel> comments)
    {
        Comments.Clear();
        foreach (ReviewCommentViewModel comment in comments)
        {
            Comments.Add(comment);
        }

        OnPropertyChanged(nameof(CommentCount));
        OnPropertyChanged(nameof(HasComments));
    }

    public void SetFocusedCommentMode(bool enabled)
    {
        if (IsFocusedCommentMode == enabled)
        {
            return;
        }

        IsFocusedCommentMode = enabled;
        OnPropertyChanged(nameof(IsFocusedCommentMode));
        OnPropertyChanged(nameof(CanOpenFocusedComment));
        OnPropertyChanged(nameof(ShowNormalCommentGutter));
        OnPropertyChanged(nameof(ShowCodeTextBox));
        OnPropertyChanged(nameof(ShowCodeTextBlock));
        OpenFocusedCommentCommand.NotifyCanExecuteChanged();
    }

    public void SetSelected(bool selected)
    {
        if (IsSelected == selected)
        {
            return;
        }

        IsSelected = selected;
        OnPropertyChanged(nameof(IsSelected));
        OnPropertyChanged(nameof(ShowCodeTextBox));
        OnPropertyChanged(nameof(ShowCodeTextBlock));
        OnPropertyChanged(nameof(ShowNormalCommentGutter));
        OnPropertyChanged(nameof(ShowPeekDefinition));
    }

    public void ConfigureFold(CodeFoldRegion? fold, bool collapsed)
    {
        FoldRegion = fold;
        IsFoldCollapsed = collapsed;
        OnPropertyChanged(nameof(FoldRegion));
        OnPropertyChanged(nameof(HasFold));
        OnPropertyChanged(nameof(IsFoldCollapsed));
        OnPropertyChanged(nameof(FoldGlyph));
        OnPropertyChanged(nameof(FoldSummary));
        OnPropertyChanged(nameof(ShowNormalCommentGutter));
        ToggleFoldCommand.NotifyCanExecuteChanged();
    }

    public void SetSearchState(
        bool isMatch,
        bool isCurrent,
        string query = "")
    {
        query = query.Trim();
        if (IsSearchMatch == isMatch &&
            IsCurrentSearchMatch == isCurrent &&
            searchQuery.Equals(query, StringComparison.Ordinal))
        {
            return;
        }

        IsSearchMatch = isMatch;
        IsCurrentSearchMatch = isCurrent;
        searchQuery = isMatch ? query : string.Empty;
        OnPropertyChanged(nameof(IsSearchMatch));
        OnPropertyChanged(nameof(IsCurrentSearchMatch));
        NotifyHighlightChanged();
    }

    public void SetDifference(string counterpart)
    {
        int prefix = 0;
        int maximum = Math.Min(CodeText.Length, counterpart.Length);
        while (prefix < maximum &&
               CodeText[prefix] == counterpart[prefix])
        {
            prefix++;
        }

        int suffix = 0;
        while (suffix < maximum - prefix &&
               CodeText[CodeText.Length - 1 - suffix] ==
               counterpart[counterpart.Length - 1 - suffix])
        {
            suffix++;
        }

        int differenceLength = Math.Max(
            0,
            CodeText.Length - prefix - suffix);
        differencePrefix = CodeText[..prefix];
        differenceText = CodeText.Substring(prefix, differenceLength);
        differenceSuffix = suffix == 0
            ? string.Empty
            : CodeText[^suffix..];
        NotifyHighlightChanged();
    }

    public void CopyDifferenceFrom(DiffLineViewModel source)
    {
        differencePrefix = source.differencePrefix;
        differenceText = source.differenceText;
        differenceSuffix = source.differenceSuffix;
        NotifyHighlightChanged();
    }

    private void NotifyHighlightChanged()
    {
        OnPropertyChanged(nameof(HighlightPrefix));
        OnPropertyChanged(nameof(HighlightText));
        OnPropertyChanged(nameof(HighlightSuffix));
        OnPropertyChanged(nameof(HighlightBrush));
        OnPropertyChanged(nameof(DisplayRuns));
    }

    public override string ToString()
    {
        string line = DisplayLineNumber;
        string kind = IsAdded ? "Added" : IsDeleted ? "Deleted" : "Code";
        return string.IsNullOrWhiteSpace(line)
            ? $"{kind}: {CodeText.Trim()}"
            : $"{kind} line {line}: {CodeText.Trim()}";
    }

    private (string Prefix, string Match, string Suffix) GetSearchParts()
    {
        if (string.IsNullOrWhiteSpace(searchQuery))
        {
            return (CodeText, string.Empty, string.Empty);
        }

        int index = CodeText.IndexOf(
            searchQuery,
            StringComparison.OrdinalIgnoreCase);
        if (index < 0)
        {
            return (CodeText, string.Empty, string.Empty);
        }

        return (
            CodeText[..index],
            CodeText.Substring(index, searchQuery.Length),
            CodeText[(index + searchQuery.Length)..]);
    }

    private IReadOnlyList<DiffCodeTextRun> CreateDisplayRuns()
    {
        var ranges = new List<DiffHighlightRange>();
        if (IsSearchMatch &&
            !string.IsNullOrWhiteSpace(searchQuery))
        {
            int start = 0;
            while (start < CodeText.Length)
            {
                int index = CodeText.IndexOf(
                    searchQuery,
                    start,
                    StringComparison.OrdinalIgnoreCase);
                if (index < 0)
                {
                    break;
                }

                ranges.Add(new DiffHighlightRange(
                    index,
                    searchQuery.Length,
                    DiffHighlightKind.Search));
                start = index + Math.Max(1, searchQuery.Length);
            }
        }
        else if (!string.IsNullOrWhiteSpace(differenceText))
        {
            ranges.Add(new DiffHighlightRange(
                differencePrefix.Length,
                differenceText.Length,
                IsAdded
                    ? DiffHighlightKind.Added
                    : DiffHighlightKind.Deleted));
        }

        if (ranges.Count == 0)
        {
            return [new DiffCodeTextRun(CodeText, DiffHighlightKind.None)];
        }

        var runs = new List<DiffCodeTextRun>();
        int position = 0;
        foreach (DiffHighlightRange range in ranges.OrderBy(range => range.Start))
        {
            if (range.Start > position)
            {
                runs.Add(new DiffCodeTextRun(
                    CodeText[position..range.Start],
                    DiffHighlightKind.None));
            }

            int length = Math.Min(
                range.Length,
                CodeText.Length - range.Start);
            if (length > 0)
            {
                runs.Add(new DiffCodeTextRun(
                    CodeText.Substring(range.Start, length),
                    range.Kind));
            }

            position = Math.Max(position, range.Start + length);
        }

        if (position < CodeText.Length)
        {
            runs.Add(new DiffCodeTextRun(
                CodeText[position..],
                DiffHighlightKind.None));
        }

        return runs;
    }
}

public enum DiffHighlightKind
{
    None,
    Search,
    Added,
    Deleted
}

public sealed record DiffCodeTextRun(
    string Text,
    DiffHighlightKind Kind);

public readonly record struct DiffHighlightRange(
    int Start,
    int Length,
    DiffHighlightKind Kind);

public sealed class SymbolLinkViewModel(
    string symbol,
    int columnIndex,
    int maximumOffset,
    IAsyncRelayCommand navigateCommand)
{
    public string Symbol { get; } = symbol;
    public IAsyncRelayCommand NavigateCommand { get; } = navigateCommand;
    public Thickness LeadingMargin { get; } = new(
        Math.Clamp(columnIndex * 7.8, 8, maximumOffset),
        0,
        0,
        0);
}
