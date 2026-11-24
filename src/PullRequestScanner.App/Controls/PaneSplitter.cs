using Microsoft.UI.Input;
using Microsoft.UI.Xaml.Controls;

namespace PullRequestScanner.App.Controls;

public sealed class PaneSplitter : Control
{
    public PaneSplitter()
    {
        DefaultStyleKey = typeof(PaneSplitter);
        ProtectedCursor = InputSystemCursor.Create(
            InputSystemCursorShape.SizeWestEast);
    }
}

public sealed class PaneSplitterSurface : Grid
{
    public PaneSplitterSurface()
    {
        ProtectedCursor = InputSystemCursor.Create(
            InputSystemCursorShape.SizeWestEast);
    }
}
