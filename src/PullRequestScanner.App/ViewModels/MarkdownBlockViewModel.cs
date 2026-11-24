using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.Input;
using CommunityToolkit.Mvvm.ComponentModel;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.App.ViewModels;

public sealed class MarkdownBlockViewModel : ObservableObject
{
    private readonly DiffLineViewModel? sourceLine;

    public MarkdownBlockViewModel(
        MarkdownBlock block,
        DiffLineViewModel? sourceLine = null,
        Func<DiffLineViewModel, Task>? composeComment = null)
    {
        Block = block;
        this.sourceLine = sourceLine;
        if (sourceLine is not null)
        {
            foreach (ReviewCommentViewModel comment in sourceLine.Comments)
            {
                Comments.Add(comment);
            }
        }

        CommentCommand = new AsyncRelayCommand(
            () => sourceLine is not null && composeComment is not null
                ? composeComment(sourceLine)
                : Task.CompletedTask,
            () => sourceLine is not null && composeComment is not null);
    }

    public MarkdownBlock Block { get; }
    public string Text => Block.Text;
    public IReadOnlyList<MarkdownInlineRun> InlineRuns =>
        Block.InlineRuns ?? [new MarkdownInlineRun(MarkdownInlineKind.Text, Text)];
    public string LineLabel => $"Line {Block.SourceLineNumber}";
    public string ListMarker => Block.ListMarker ?? string.Empty;
    public bool IsHeading1 => Block.Kind == MarkdownBlockKind.Heading1;
    public bool IsHeading2 => Block.Kind == MarkdownBlockKind.Heading2;
    public bool IsHeading3 => Block.Kind == MarkdownBlockKind.Heading3;
    public bool IsHeading4 => Block.Kind == MarkdownBlockKind.Heading4;
    public bool IsHeading5 => Block.Kind == MarkdownBlockKind.Heading5;
    public bool IsHeading6 => Block.Kind == MarkdownBlockKind.Heading6;
    public bool IsBullet => Block.Kind == MarkdownBlockKind.Bullet;
    public bool IsNumbered => Block.Kind == MarkdownBlockKind.Numbered;
    public bool IsQuote => Block.Kind == MarkdownBlockKind.Quote;
    public bool IsCode => Block.Kind == MarkdownBlockKind.Code;
    public bool IsHorizontalRule => Block.Kind == MarkdownBlockKind.HorizontalRule;
    public bool IsTaskChecked => Block.Kind == MarkdownBlockKind.TaskChecked;
    public bool IsTaskUnchecked => Block.Kind == MarkdownBlockKind.TaskUnchecked;
    public bool IsTable => Block.Kind == MarkdownBlockKind.Table;
    public bool IsAdded => sourceLine?.IsAdded == true;
    public bool IsDeleted => sourceLine?.IsDeleted == true;
    public bool IsParagraph => Block.Kind == MarkdownBlockKind.Paragraph;
    public bool CanComment => sourceLine is not null;
    public ObservableCollection<ReviewCommentViewModel> Comments { get; } = [];
    public bool HasComments => Comments.Count > 0;
    public IAsyncRelayCommand CommentCommand { get; }

    public void RefreshComments()
    {
        Comments.Clear();
        if (sourceLine is not null)
        {
            foreach (ReviewCommentViewModel comment in sourceLine.Comments)
            {
                Comments.Add(comment);
            }
        }

        OnPropertyChanged(nameof(HasComments));
    }
}
