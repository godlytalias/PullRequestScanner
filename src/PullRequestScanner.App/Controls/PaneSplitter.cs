using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;

namespace PullRequestScanner.App.Controls;

public sealed class PaneSplitter : Control
{
    private readonly InputSystemCursor resizeCursor =
        InputSystemCursor.Create(InputSystemCursorShape.SizeWestEast);

    public PaneSplitter()
    {
        DefaultStyleKey = typeof(PaneSplitter);
        Loaded += ApplyResizeCursor;
        PointerEntered += ApplyResizeCursor;
    }

    protected override void OnApplyTemplate()
    {
        base.OnApplyTemplate();
        ProtectedCursor = resizeCursor;
    }

    private void ApplyResizeCursor(
        object sender,
        RoutedEventArgs args) =>
        ProtectedCursor = resizeCursor;

    private void ApplyResizeCursor(
        object sender,
        PointerRoutedEventArgs args) =>
        ProtectedCursor = resizeCursor;
}

public sealed class PaneSplitterSurface : Grid
{
    private readonly InputSystemCursor resizeCursor =
        InputSystemCursor.Create(InputSystemCursorShape.SizeWestEast);

    public PaneSplitterSurface()
    {
        Loaded += ApplyResizeCursor;
        PointerEntered += ApplyResizeCursor;
    }

    private void ApplyResizeCursor(
        object sender,
        RoutedEventArgs args) =>
        ProtectedCursor = resizeCursor;

    private void ApplyResizeCursor(
        object sender,
        PointerRoutedEventArgs args) =>
        ProtectedCursor = resizeCursor;
}
