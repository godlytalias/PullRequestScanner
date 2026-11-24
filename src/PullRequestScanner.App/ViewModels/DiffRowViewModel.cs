namespace PullRequestScanner.App.ViewModels;

public sealed class DiffRowViewModel
{
    public DiffRowViewModel(
        DiffLineViewModel inline)
    {
        Inline = inline;
        Hunk = inline.IsHunk ? inline : null;
    }

    public DiffRowViewModel(
        DiffLineViewModel? hunk,
        DiffLineViewModel? left,
        DiffLineViewModel? right)
    {
        Hunk = hunk;
        Left = left;
        Right = right;
    }

    public DiffLineViewModel? Inline { get; }
    public DiffLineViewModel? Hunk { get; }
    public DiffLineViewModel? Left { get; }
    public DiffLineViewModel? Right { get; }
    public bool IsInline => Inline is not null && !IsHunk;
    public bool IsHunk => Hunk is not null;
    public bool IsSideBySide => !IsInline && !IsHunk;
    public bool HasLeft => Left is not null;
    public bool HasRight => Right is not null;
    public bool IsLeftEmpty => !HasLeft && HasRight;
    public bool IsRightEmpty => HasLeft && !HasRight;
    public string HunkText => Hunk?.CodeText ?? string.Empty;

    public bool Contains(DiffLineViewModel line) =>
        ReferenceEquals(Inline, line) ||
        ReferenceEquals(Hunk, line) ||
        ReferenceEquals(Left, line) ||
        ReferenceEquals(Right, line) ||
        ReferenceEquals(Inline?.Line, line.Line) ||
        ReferenceEquals(Hunk?.Line, line.Line) ||
        ReferenceEquals(Left?.Line, line.Line) ||
        ReferenceEquals(Right?.Line, line.Line);

    public override string ToString()
    {
        if (IsHunk)
        {
            return $"Diff hunk: {HunkText}";
        }

        string left = Left?.ToString() ?? "No old line";
        string right = Right?.ToString() ?? "No new line";
        return $"{left}; {right}";
    }
}
