using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.UI.Xaml.Input;
using PullRequestScanner.App.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Dispatching;
using PullRequestScanner.Core.Models;
using Windows.Storage;
using Windows.Storage.Pickers;
using Microsoft.UI.Text;
using WinRT.Interop;

namespace PullRequestScanner.App;

public sealed partial class MainPage : Page
{
    private ReviewSessionViewModel? subscribedReview;
    private DispatcherQueueTimer? reviewRefreshTimer;
    private bool isRefreshingReviews;
    private bool isFocusedCommentDialogOpen;
    private bool isDescriptionDialogOpen;
    private bool isInitialized;
    private bool isSynchronizingNavigation;
    private readonly Dictionary<TextBlock, DiffCodeTextBinding> diffCodeTextBindings = [];
    private readonly List<(double Y, DiffChangeMarkerViewModel Marker)> markerHitTargets = [];
    private double changedFilesSplitterX = double.NaN;
    private double reviewInspectorSplitterX = double.NaN;

    public MainViewModel ViewModel { get; }

    public MainPage()
    {
        ViewModel = new MainViewModel(App.Services.ReviewSessions);
        InitializeComponent();
        AddHandler(
            UIElement.PointerMovedEvent,
            new PointerEventHandler(Workspace_PointerMoved),
            false);
        AddHandler(
            UIElement.PointerReleasedEvent,
            new PointerEventHandler(Workspace_PointerReleased),
            false);
        AddHandler(
            UIElement.PointerCanceledEvent,
            new PointerEventHandler(Workspace_PointerReleased),
            false);
        Loaded += MainPage_Loaded;
        Unloaded += MainPage_Unloaded;
    }
    public void ShowOpenReviewPanel()
    {
        if (ViewModel.IsCodeViewExpanded)
        {
            ViewModel.IsCodeViewExpanded = false;
        }

        if (ViewModel.ActiveReview.HasActiveReview)
        {
            _ = ShowOpenReviewDialogAsync();
            return;
        }

        ViewModel.ShowOpenReviewPanelCommand.Execute(null);
        DispatcherQueue.TryEnqueue(() =>
        {
            PullRequestUrlTextBox.Focus(FocusState.Programmatic);
            PullRequestUrlTextBox.SelectAll();
        });
    }

    private void OpenReviewAccelerator_Invoked(
        KeyboardAccelerator sender,
        KeyboardAcceleratorInvokedEventArgs args)
    {
        ShowOpenReviewPanel();
        args.Handled = true;
    }

    private async Task ShowOpenReviewDialogAsync()
    {
        OpenReviewDialog.XamlRoot = XamlRoot;
        await OpenReviewDialog.ShowAsync();
    }

    private async void OpenReviewDialog_PrimaryButtonClick(
        ContentDialog sender,
        ContentDialogButtonClickEventArgs args)
    {
        ContentDialogButtonClickDeferral deferral = args.GetDeferral();
        try
        {
            if (!ViewModel.CanOpenReview)
            {
                args.Cancel = true;
                return;
            }

            await ViewModel.OpenReviewCommand.ExecuteAsync(null);
            args.Cancel = ViewModel.HasError ||
                ViewModel.AuthenticationRequired;
        }
        finally
        {
            deferral.Complete();
        }
    }

    private async void PullRequestUrlTextBox_KeyDown(
        object sender,
        KeyRoutedEventArgs e)
    {
        if (e.Key != Windows.System.VirtualKey.Enter ||
            !ViewModel.CanOpenReview)
        {
            return;
        }

        e.Handled = true;
        await ViewModel.OpenReviewCommand.ExecuteAsync(null);
    }

    private void FindAccelerator_Invoked(
        KeyboardAccelerator sender,
        KeyboardAcceleratorInvokedEventArgs args)
    {
        FileSearchTextBox.Focus(FocusState.Programmatic);
        FileSearchTextBox.SelectAll();
        args.Handled = true;
    }

    private void NextSearchAccelerator_Invoked(
        KeyboardAccelerator sender,
        KeyboardAcceleratorInvokedEventArgs args)
    {
        if (ViewModel.ActiveReview.CanMoveSearchNext)
        {
            ViewModel.ActiveReview.NextSearchResultCommand.Execute(null);
        }

        args.Handled = true;
    }

    private void PreviousSearchAccelerator_Invoked(
        KeyboardAccelerator sender,
        KeyboardAcceleratorInvokedEventArgs args)
    {
        if (ViewModel.ActiveReview.CanMoveSearchPrevious)
        {
            ViewModel.ActiveReview.PreviousSearchResultCommand.Execute(null);
        }

        args.Handled = true;
    }

    private void NextChangeAccelerator_Invoked(
        KeyboardAccelerator sender,
        KeyboardAcceleratorInvokedEventArgs args)
    {
        ViewModel.ActiveReview.NextChangeCommand.Execute(null);
        args.Handled = true;
    }

    private void PreviousChangeAccelerator_Invoked(
        KeyboardAccelerator sender,
        KeyboardAcceleratorInvokedEventArgs args)
    {
        ViewModel.ActiveReview.PreviousChangeCommand.Execute(null);
        args.Handled = true;
    }

    private async void RefreshAccelerator_Invoked(
        KeyboardAccelerator sender,
        KeyboardAcceleratorInvokedEventArgs args)
    {
        await ViewModel.ActiveReview.RefreshCommand.ExecuteAsync(null);
        args.Handled = true;
    }

    private async void PeekDefinitionAccelerator_Invoked(
        KeyboardAccelerator sender,
        KeyboardAcceleratorInvokedEventArgs args)
    {
        if (ViewModel.ActiveReview.SelectedDiffLine is not null &&
            ViewModel.ActiveReview.SelectedDiffLine.CanNavigate)
        {
            await ViewModel.ActiveReview.SelectedDiffLine
                .PeekDefinitionCommand
                .ExecuteAsync(null);
        }

        args.Handled = true;
    }

    private void FocusModeAccelerator_Invoked(
        KeyboardAccelerator sender,
        KeyboardAcceleratorInvokedEventArgs args)
    {
        ViewModel.IsCodeViewExpanded = !ViewModel.IsCodeViewExpanded;
        args.Handled = true;
    }

    private void FocusModeExitAccelerator_Invoked(
        KeyboardAccelerator sender,
        KeyboardAcceleratorInvokedEventArgs args)
    {
        if (!isFocusedCommentDialogOpen &&
            !isDescriptionDialogOpen &&
            ViewModel.IsCodeViewExpanded)
        {
            ViewModel.IsCodeViewExpanded = false;
            args.Handled = true;
            return;
        }

        args.Handled = false;
    }

    private void ExpandCodeViewButton_Click(
        object sender,
        RoutedEventArgs e) =>
        ViewModel.IsCodeViewExpanded = !ViewModel.IsCodeViewExpanded;

    private async void SaveCommentAccelerator_Invoked(
        KeyboardAccelerator sender,
        KeyboardAcceleratorInvokedEventArgs args)
    {
        if (FocusManager.GetFocusedElement(XamlRoot) is not
            DependencyObject focused)
        {
            args.Handled = false;
            return;
        }

        if (focused is FrameworkElement focusedElement &&
            focusedElement.DataContext is ReviewCommentViewModel comment &&
            comment.CanReply &&
            !string.IsNullOrWhiteSpace(comment.ReplyText))
        {
            await comment.ReplyCommand.ExecuteAsync(null);
            args.Handled = true;
            return;
        }

        bool isFocusedComposer = isFocusedCommentDialogOpen &&
            IsDescendantOf(focused, FocusedCommentDialog);
        bool isInspectorComposer =
            ReferenceEquals(focused, DraftCommentTextBox) ||
            ReferenceEquals(focused, DraftSuggestionTextBox);
        if ((isFocusedComposer || isInspectorComposer) &&
            ViewModel.ActiveReview.CanSaveCommentLocally)
        {
            await ViewModel.ActiveReview.AddCommentCommand.ExecuteAsync(null);
            if (isFocusedComposer)
            {
                FocusedCommentDialog.Hide();
            }

            args.Handled = true;
            return;
        }

        args.Handled = false;
    }

    private async void PublishAccelerator_Invoked(
        KeyboardAccelerator sender,
        KeyboardAcceleratorInvokedEventArgs args)
    {
        if (ViewModel.ActiveReview.CanPublishPendingComments)
        {
            await ViewModel.ActiveReview.PublishCommentsCommand.ExecuteAsync(null);
            args.Handled = true;
            return;
        }

        args.Handled = false;
    }

    private static bool IsDescendantOf(
        DependencyObject element,
        DependencyObject ancestor)
    {
        DependencyObject? current = element;
        while (current is not null)
        {
            if (ReferenceEquals(current, ancestor))
            {
                return true;
            }

            current = VisualTreeHelper.GetParent(current);
        }

        return false;
    }

    private async void MainPage_Loaded(object sender, RoutedEventArgs e)
    {
        if (!isInitialized)
        {
            await ViewModel.InitializeAsync();
            isInitialized = true;
        }

        ViewModel.PropertyChanged -= ViewModel_PropertyChanged;
        ViewModel.PropertyChanged += ViewModel_PropertyChanged;
        AttachActiveReview();
        ApplyReviewContentVisibility();
        SyncInspectorTabs();
        QueueNavigationSelectionSync();
        reviewRefreshTimer ??= DispatcherQueue.CreateTimer();
        reviewRefreshTimer.Interval = TimeSpan.FromMinutes(5);
        reviewRefreshTimer.Tick -= ReviewRefreshTimer_Tick;
        reviewRefreshTimer.Tick += ReviewRefreshTimer_Tick;
        reviewRefreshTimer.Start();
    }

    private async void ReviewNavigationView_SelectionChanged(
        NavigationView sender,
        NavigationViewSelectionChangedEventArgs args)
    {
        if (isSynchronizingNavigation)
        {
            return;
        }

        if (ViewModel.IsActivityViewVisible)
        {
            return;
        }

        if (args.SelectedItem is ReviewSessionViewModel review)
        {
            await SelectReviewAsync(review);
        }
    }

    private async void ReviewNavigationView_ItemInvoked(
        NavigationView sender,
        NavigationViewItemInvokedEventArgs args)
    {
        if (ViewModel.IsActivityViewVisible)
        {
            QueueNavigationSelectionSync();
            return;
        }

        if (args.InvokedItem is ReviewSessionViewModel review)
        {
            await SelectReviewAsync(review);
        }
    }

    private async Task SelectReviewAsync(ReviewSessionViewModel review)
    {
        if (!ReferenceEquals(ViewModel.ActiveReview, review))
        {
            await ViewModel.SelectReviewAsync(review);
        }

        QueueNavigationSelectionSync();
    }

    private void QueueNavigationSelectionSync()
    {
        DispatcherQueue.TryEnqueue(
            DispatcherQueuePriority.Low,
            SyncNavigationSelection);
    }

    private void SyncNavigationSelection()
    {
        isSynchronizingNavigation = true;
        try
        {
            ReviewSessionViewModel? selected =
                ViewModel.SelectedOpenReview;
            ReviewNavigationView.SelectedItem = selected;
        }
        finally
        {
            isSynchronizingNavigation = false;
        }
    }

    private async void OpenReviewActivityButton_Click(
        object sender,
        RoutedEventArgs e)
    {
        ViewModel.ShowReviewActivityCommand.Execute(null);
        ApplyReviewContentVisibility();
        await EmbeddedReviewActivityPage.RefreshAsync();
    }

    private void ReviewActivityPage_BackRequested(
        object? sender,
        EventArgs e)
    {
        ViewModel.HideReviewActivityCommand.Execute(null);
        ApplyReviewContentVisibility();
    }

    private void ApplyReviewContentVisibility()
    {
        ReviewWorkspace.Visibility =
            ViewModel.IsActivityViewVisible
                ? Visibility.Collapsed
                : Visibility.Visible;
        EmbeddedReviewActivityPage.Visibility =
            ViewModel.IsActivityViewVisible
                ? Visibility.Visible
                : Visibility.Collapsed;
        SyncNavigationSelection();
        if (ReviewNavigationView.Content is FrameworkElement content)
        {
            content.UpdateLayout();
        }
    }

    private void InspectorTab_Click(
        object sender,
        RoutedEventArgs e)
    {
        if (ReferenceEquals(sender, CommentsInspectorTab))
        {
            ViewModel.ActiveReview.ShowCommentsInspectorCommand.Execute(null);
        }
        else if (ReferenceEquals(sender, OverviewInspectorTab))
        {
            ViewModel.ActiveReview.ShowOverviewInspectorCommand.Execute(null);
        }
        else if (ReferenceEquals(sender, NotesInspectorTab))
        {
            ViewModel.ActiveReview.ShowNotesInspectorCommand.Execute(null);
        }

        SyncInspectorTabs();
    }

    private void SyncInspectorTabs()
    {
        CommentsInspectorTab.IsChecked =
            ViewModel.ActiveReview.IsCommentsInspectorVisible;
        OverviewInspectorTab.IsChecked =
            ViewModel.ActiveReview.IsOverviewInspectorVisible;
        NotesInspectorTab.IsChecked =
            ViewModel.ActiveReview.IsNotesInspectorVisible;
    }

    private async void FileTreeView_SelectionChanged(
        TreeView sender,
        TreeViewSelectionChangedEventArgs args)
    {
        if (args.AddedItems.FirstOrDefault() is FileTreeNodeViewModel node)
        {
            await ViewModel.ActiveReview.SelectTreeNodeAsync(node);
        }
    }

    private async void DiffContextComboBox_SelectionChanged(
        object sender,
        SelectionChangedEventArgs e)
    {
        if (e.AddedItems.FirstOrDefault() is int contextLines)
        {
            await ViewModel.ActiveReview.ChangeContextAsync(contextLines);
        }
    }

    private void ChangedFilesSplitter_Loaded(
        object sender,
        RoutedEventArgs e)
    {
        if (sender is UIElement element)
        {
            element.AddHandler(
                UIElement.PointerPressedEvent,
                new PointerEventHandler(ChangedFilesSplitter_PointerPressed),
                true);
        }
    }

    private void ChangedFilesSplitter_Unloaded(
        object sender,
        RoutedEventArgs e)
    {
        if (sender is UIElement element)
        {
            element.RemoveHandler(
                UIElement.PointerPressedEvent,
                new PointerEventHandler(ChangedFilesSplitter_PointerPressed));
        }
    }

    private void ReviewInspectorSplitter_Loaded(
        object sender,
        RoutedEventArgs e)
    {
        if (sender is UIElement element)
        {
            element.AddHandler(
                UIElement.PointerPressedEvent,
                new PointerEventHandler(ReviewInspectorSplitter_PointerPressed),
                true);
        }
    }

    private void ReviewInspectorSplitter_Unloaded(
        object sender,
        RoutedEventArgs e)
    {
        if (sender is UIElement element)
        {
            element.RemoveHandler(
                UIElement.PointerPressedEvent,
                new PointerEventHandler(ReviewInspectorSplitter_PointerPressed));
        }
    }

    private void ChangedFilesSplitter_PointerPressed(
        object sender,
        PointerRoutedEventArgs e)
    {
        changedFilesSplitterX = e.GetCurrentPoint(null).Position.X;
        CapturePointer(e.Pointer);
        e.Handled = true;
    }

    private void Workspace_PointerMoved(
        object sender,
        PointerRoutedEventArgs e)
    {
        double x = e.GetCurrentPoint(null).Position.X;
        if (!double.IsNaN(changedFilesSplitterX))
        {
            ViewModel.ResizeChangedFilesPane(x - changedFilesSplitterX);
            changedFilesSplitterX = x;
            e.Handled = true;
        }
        else if (!double.IsNaN(reviewInspectorSplitterX))
        {
            ViewModel.ResizeReviewInspectorPane(
                x - reviewInspectorSplitterX);
            reviewInspectorSplitterX = x;
            e.Handled = true;
        }
    }

    private void Workspace_PointerReleased(
        object sender,
        PointerRoutedEventArgs e)
    {
        if (double.IsNaN(changedFilesSplitterX) &&
            double.IsNaN(reviewInspectorSplitterX))
        {
            return;
        }

        changedFilesSplitterX = double.NaN;
        reviewInspectorSplitterX = double.NaN;
        ReleasePointerCapture(e.Pointer);
    }

    private void ReviewInspectorSplitter_PointerPressed(
        object sender,
        PointerRoutedEventArgs e)
    {
        reviewInspectorSplitterX = e.GetCurrentPoint(null).Position.X;
        CapturePointer(e.Pointer);
        e.Handled = true;
    }

    private void ChangedFilesSplitter_KeyDown(
        object sender,
        KeyRoutedEventArgs e) =>
        HandleSplitterKey(e, ViewModel.ResizeChangedFilesPane);

    private void ReviewInspectorSplitter_KeyDown(
        object sender,
        KeyRoutedEventArgs e) =>
        HandleSplitterKey(e, ViewModel.ResizeReviewInspectorPane);

    private static void HandleSplitterKey(
        KeyRoutedEventArgs e,
        Action<double> resize)
    {
        if (e.Key == Windows.System.VirtualKey.Left ||
            e.Key == Windows.System.VirtualKey.Right)
        {
            double direction = e.Key == Windows.System.VirtualKey.Right
                ? 1
                : -1;
            resize(direction * 8);
            e.Handled = true;
        }
    }

    private void ReviewErrorInfoBar_Closed(
        InfoBar sender,
        InfoBarClosedEventArgs args) =>
        ViewModel.ActiveReview.ErrorMessage = string.Empty;

    private void GlobalErrorInfoBar_Closed(
        InfoBar sender,
        InfoBarClosedEventArgs args) =>
        ViewModel.ErrorMessage = string.Empty;

    private async void BrowseRepositoryButton_Click(
        object sender,
        RoutedEventArgs e)
    {
        if (App.MainWindow is null)
        {
            ViewModel.ErrorMessage = "The application window is not available for the folder picker.";
            return;
        }

        var picker = new FolderPicker
        {
            SuggestedStartLocation = PickerLocationId.ComputerFolder,
            ViewMode = PickerViewMode.List
        };
        picker.FileTypeFilter.Add("*");
        InitializeWithWindow.Initialize(
            picker,
            WindowNative.GetWindowHandle(App.MainWindow));
        try
        {
            StorageFolder? folder = await picker.PickSingleFolderAsync();
            if (folder is not null)
            {
                ViewModel.LocalRepositoryPath = folder.Path;
                ViewModel.ErrorMessage = string.Empty;
            }
        }
        catch (Exception exception) when (
            exception is UnauthorizedAccessException
                or InvalidOperationException
                or COMException)
        {
            ViewModel.ErrorMessage = $"Could not open the repository picker: {exception.Message}";
        }
    }

    private async void IterationComboBox_SelectionChanged(
        object sender,
        SelectionChangedEventArgs e)
    {
        if (e.AddedItems.FirstOrDefault() is PullRequestIteration iteration)
        {
            await ViewModel.ActiveReview.SelectIterationAsync(iteration);
        }
    }

    private async void ReviewedPullRequests_SelectionChanged(
        object sender,
        SelectionChangedEventArgs e)
    {
        if (e.AddedItems.FirstOrDefault() is ReviewSessionViewModel review)
        {
            await ViewModel.SelectReviewAsync(review);
        }
    }

    private void DiffCodeTextBlock_Loaded(
        object sender,
        RoutedEventArgs e)
    {
        if (sender is TextBlock textBlock)
        {
            AttachDiffCodeTextBinding(textBlock);
        }
    }

    private void DiffCodeTextBlock_DataContextChanged(
        FrameworkElement sender,
        DataContextChangedEventArgs e)
    {
        if (sender is TextBlock textBlock)
        {
            AttachDiffCodeTextBinding(textBlock);
        }
    }

    private void DiffCodeTextBlock_Unloaded(
        object sender,
        RoutedEventArgs e)
    {
        if (sender is TextBlock textBlock)
        {
            DetachDiffCodeTextBinding(textBlock);
        }
    }

    private void DiffCodeTextBlock_SelectionChanged(
        object sender,
        RoutedEventArgs e)
    {
        if (sender is TextBlock textBlock &&
            textBlock.DataContext is DiffLineViewModel line)
        {
            ViewModel.ActiveReview.SelectLine(line);
            string selected = textBlock.SelectedText;
            int first = GetSelectedTextOffset(
                textBlock,
                textBlock.SelectionStart);
            int second = GetSelectedTextOffset(
                textBlock,
                textBlock.SelectionEnd);
            int start = Math.Min(first, second);
            line.SelectCodeRange(
                start,
                Math.Max(selected.Length, Math.Abs(second - first)));
        }
    }

    private void DiffCodeTextBlock_Tapped(
        object sender,
        TappedRoutedEventArgs e)
    {
        if (sender is TextBlock textBlock &&
            textBlock.DataContext is DiffLineViewModel line)
        {
            ViewModel.ActiveReview.SelectLine(line);
        }
    }

    private void DiffCodeTextBlock_RightTapped(
        object sender,
        RightTappedRoutedEventArgs e)
    {
        if (sender is not TextBlock textBlock ||
            textBlock.DataContext is not DiffLineViewModel line ||
            line.NavigationSymbols.Count == 0)
        {
            return;
        }

        ViewModel.ActiveReview.SelectLine(line);
        var flyout = new MenuFlyout();
        foreach (SymbolLinkViewModel symbol in line.NavigationSymbols)
        {
            flyout.Items.Add(new MenuFlyoutItem
            {
                Text = $"Peek {symbol.Symbol}",
                Command = symbol.NavigateCommand
            });
        }

        flyout.ShowAt(
            textBlock,
            e.GetPosition(textBlock));
        e.Handled = true;
    }

    private async void ViewPullRequestDescriptionButton_Click(
        object sender,
        RoutedEventArgs e)
    {
        if (!ViewModel.ActiveReview.HasDescription ||
            isDescriptionDialogOpen)
        {
            return;
        }

        DescriptionDialog.XamlRoot = XamlRoot;
        isDescriptionDialogOpen = true;
        try
        {
            await DescriptionDialog.ShowAsync();
        }
        catch (Exception exception) when (
            exception is InvalidOperationException
                or COMException)
        {
            ViewModel.ErrorMessage =
                $"Could not open the pull request description: {exception.Message}";
        }
        finally
        {
            isDescriptionDialogOpen = false;
        }
    }

    private async void CompleteReviewAction_Click(
        object sender,
        RoutedEventArgs e)
    {
        CompleteReviewDialog.XamlRoot = XamlRoot;
        await CompleteReviewDialog.ShowAsync();
    }

    private void MarkdownRichTextBlock_Loaded(
        object sender,
        RoutedEventArgs e)
    {
        if (sender is not RichTextBlock textBlock ||
            textBlock.DataContext is not MarkdownBlockViewModel block)
        {
            return;
        }

        textBlock.Blocks.Clear();
        var paragraph = new Paragraph();
        AddMarkdownInlines(paragraph.Inlines, block);
        textBlock.Blocks.Add(paragraph);
    }

    private void MarkdownTextBlock_Loaded(
        object sender,
        RoutedEventArgs e)
    {
        if (sender is not TextBlock textBlock ||
            textBlock.DataContext is not MarkdownBlockViewModel block)
        {
            return;
        }

        textBlock.Inlines.Clear();
        AddMarkdownInlines(textBlock.Inlines, block);
    }

    private static void AddMarkdownInlines(
        InlineCollection inlines,
        MarkdownBlockViewModel block)
    {
        foreach (MarkdownInlineRun inline in block.InlineRuns)
        {
            switch (inline.Kind)
            {
                case MarkdownInlineKind.Bold:
                    inlines.Add(new Run
                    {
                        FontWeight = FontWeights.SemiBold,
                        Text = inline.Text
                    });
                    break;
                case MarkdownInlineKind.Code:
                    inlines.Add(new Run
                    {
                        FontFamily = new FontFamily("Cascadia Mono"),
                        Foreground = Application.Current.Resources[
                            "AccentTextFillColorPrimaryBrush"] as Brush,
                        Text = inline.Text
                    });
                    break;
                case MarkdownInlineKind.Link when inline.NavigateUri is not null:
                    var link = new Hyperlink
                    {
                        NavigateUri = inline.NavigateUri
                    };
                    link.Inlines.Add(new Run { Text = inline.Text });
                    inlines.Add(link);
                    break;
                default:
                    inlines.Add(new Run { Text = inline.Text });
                    break;
            }
        }
    }

    private async void CompleteReviewDialog_PrimaryButtonClick(
        ContentDialog sender,
        ContentDialogButtonClickEventArgs args)
    {
        ContentDialogButtonClickDeferral deferral = args.GetDeferral();
        try
        {
            await ViewModel.CompleteReviewCommand.ExecuteAsync(null);
        }
        finally
        {
            deferral.Complete();
        }
    }

    private async Task ShowFocusedCommentDialogAsync(
        DiffLineViewModel line,
        bool allowNormalLayout = false)
    {
        if ((!allowNormalLayout && !ViewModel.IsCodeViewExpanded) ||
            isFocusedCommentDialogOpen)
        {
            return;
        }

        ViewModel.ActiveReview.SelectLine(line);
        ViewModel.ActiveReview.PrepareFocusedComment();
        FocusedCommentDialog.XamlRoot = XamlRoot;
        isFocusedCommentDialogOpen = true;
        try
        {
            await FocusedCommentDialog.ShowAsync();
        }
        catch (Exception exception) when (
            exception is InvalidOperationException
                or COMException)
        {
            ViewModel.ActiveReview.ErrorMessage =
                $"Could not open the comment composer: {exception.Message}";
        }
        finally
        {
            isFocusedCommentDialogOpen = false;
        }
    }

    private async void FocusedCommentDialog_PrimaryButtonClick(
        ContentDialog sender,
        ContentDialogButtonClickEventArgs args)
    {
        ContentDialogButtonClickDeferral deferral = args.GetDeferral();
        try
        {
            if (!ViewModel.ActiveReview.CanSaveCommentLocally)
            {
                args.Cancel = true;
                return;
            }

            await ViewModel.ActiveReview.AddCommentCommand.ExecuteAsync(null);
        }
        finally
        {
            deferral.Complete();
        }
    }

    private void AttachDiffCodeTextBinding(TextBlock textBlock)
    {
        DetachDiffCodeTextBinding(textBlock);
        if (textBlock.DataContext is not DiffLineViewModel line)
        {
            return;
        }

        PropertyChangedEventHandler changed = (_, _) =>
            RenderDiffCodeText(textBlock, line);
        line.PropertyChanged += changed;
        diffCodeTextBindings[textBlock] = new DiffCodeTextBinding(
            line,
            changed,
            []);
        RenderDiffCodeText(textBlock, line);
    }

    private void DetachDiffCodeTextBinding(TextBlock textBlock)
    {
        if (diffCodeTextBindings.Remove(
                textBlock,
                out DiffCodeTextBinding? binding))
        {
            binding.Line.PropertyChanged -= binding.Changed;
        }
    }

    private void RenderDiffCodeText(
        TextBlock textBlock,
        DiffLineViewModel line)
    {
        if (!diffCodeTextBindings.TryGetValue(
                textBlock,
                out DiffCodeTextBinding? binding))
        {
            return;
        }

        textBlock.Inlines.Clear();
        var offsets = new List<(Run Run, int Start)>();
        int offset = 0;
        foreach (DiffCodeTextRun segment in line.DisplayRuns)
        {
            Brush? foreground = segment.Kind switch
            {
                DiffHighlightKind.Search => Application.Current.Resources[
                    "AccentTextFillColorPrimaryBrush"] as Brush,
                DiffHighlightKind.Added => Application.Current.Resources[
                    "AddedDiffLineStrokeBrush"] as Brush,
                DiffHighlightKind.Deleted => Application.Current.Resources[
                    "DeletedDiffLineStrokeBrush"] as Brush,
                _ => Application.Current.Resources[
                    "TextFillColorPrimaryBrush"] as Brush
            };
            var run = new Run
            {
                FontWeight = segment.Kind == DiffHighlightKind.None
                    ? FontWeights.Normal
                    : FontWeights.SemiBold,
                Foreground = foreground ?? textBlock.Foreground,
                Text = segment.Text
            };
            offsets.Add((run, offset));
            offset += segment.Text.Length;
            textBlock.Inlines.Add(run);
        }

        diffCodeTextBindings[textBlock] = binding with
        {
            RunOffsets = offsets
        };
    }

    private int GetSelectedTextOffset(
        TextBlock textBlock,
        TextPointer? pointer)
    {
        if (pointer is null)
        {
            return 0;
        }

        if (diffCodeTextBindings.TryGetValue(
                textBlock,
                out DiffCodeTextBinding? binding) &&
            pointer.Parent is Run selectedRun)
        {
            foreach ((Run run, int start) in binding.RunOffsets)
            {
                if (ReferenceEquals(run, selectedRun))
                {
                    return start + pointer.Offset;
                }
            }
        }

        string selected = textBlock.SelectedText;
        if (textBlock.DataContext is DiffLineViewModel line &&
            !string.IsNullOrEmpty(selected))
        {
            int index = line.CodeText.IndexOf(
                selected,
                StringComparison.Ordinal);
            return index >= 0 ? index : 0;
        }

        return pointer.Offset;
    }

    private sealed record DiffCodeTextBinding(
        DiffLineViewModel Line,
        PropertyChangedEventHandler Changed,
        IReadOnlyList<(Run Run, int Start)> RunOffsets);

    private void RequestFocusedCommentDialog(DiffLineViewModel line) =>
        DispatcherQueue.TryEnqueue(
            () => _ = ShowFocusedCommentDialogAsync(line));

    private void ViewModel_PropertyChanged(
        object? sender,
        PropertyChangedEventArgs args)
    {
        if (args.PropertyName ==
            nameof(MainViewModel.IsActivityViewVisible))
        {
            ApplyReviewContentVisibility();
        }

        if (args.PropertyName == nameof(MainViewModel.ActiveReview) ||
            args.PropertyName == nameof(MainViewModel.SelectedOpenReview))
        {
            if (args.PropertyName == nameof(MainViewModel.ActiveReview))
            {
                AttachActiveReview();
                SyncInspectorTabs();
            }

            QueueNavigationSelectionSync();
        }
    }

    private void AttachActiveReview()
    {
        if (subscribedReview is not null)
        {
            subscribedReview.ScrollToLineRequested -= Review_ScrollToLineRequested;
            subscribedReview.DiffMarkersChanged -= Review_DiffMarkersChanged;
            subscribedReview.CommentComposerRequested -= Review_CommentComposerRequested;
        }

        subscribedReview = ViewModel.ActiveReview;
        subscribedReview.ScrollToLineRequested += Review_ScrollToLineRequested;
        subscribedReview.DiffMarkersChanged += Review_DiffMarkersChanged;
        subscribedReview.CommentComposerRequested += Review_CommentComposerRequested;
        RenderDiffMarkers();
    }

    private async void Review_CommentComposerRequested(
        DiffLineViewModel line) =>
        await ShowFocusedCommentDialogAsync(
            line,
            allowNormalLayout: true);

    private void Review_ScrollToLineRequested(DiffLineViewModel line)
    {
        DispatcherQueue.TryEnqueue(
            () => _ = ScrollToLineAsync(line));
    }

    private async Task ScrollToLineAsync(DiffLineViewModel line)
    {
        for (int attempt = 0; attempt < 8; attempt++)
        {
            ListViewBase activeList;
            object item;
            if (ViewModel.ActiveReview.IsInlineDiff)
            {
                activeList = InlineDiffListView;
                item = line;
            }
            else
            {
                DiffRowViewModel? row = ViewModel.ActiveReview.FindDiffRow(line);
                if (row is null)
                {
                    await Task.Delay(25);
                    continue;
                }

                activeList = SideBySideDiffListView;
                item = row;
            }

            activeList.ScrollIntoView(item, ScrollIntoViewAlignment.Leading);
            activeList.UpdateLayout();
            if (activeList.ContainerFromItem(item) is ListViewItem container)
            {
                container.StartBringIntoView();
                return;
            }

            await Task.Delay(25);
        }
    }

    private void Review_DiffMarkersChanged(object? sender, EventArgs args) =>
        DispatcherQueue.TryEnqueue(RenderDiffMarkers);

    private void DiffMarkerCanvas_SizeChanged(
        object sender,
        SizeChangedEventArgs e)
    {
        RenderDiffMarkers();
    }

    private void RenderDiffMarkers()
    {
        DiffMarkerCanvas.Children.Clear();
        markerHitTargets.Clear();
        if (DiffMarkerCanvas.ActualHeight <= 0 ||
            ViewModel.ActiveReview.DiffMarkers.Count == 0)
        {
            return;
        }

        var positioned = ViewModel.ActiveReview.DiffMarkers
            .Select(marker => (
                Marker: marker,
                Y: Math.Clamp(
                marker.Position *
                Math.Max(0, DiffMarkerCanvas.ActualHeight - 4),
                0,
                Math.Max(0, DiffMarkerCanvas.ActualHeight - 4))))
            .OrderBy(item => item.Y)
            .ToArray();
        var coalesced = new List<(DiffChangeMarkerViewModel Marker, double Y)>();
        foreach (var item in positioned)
        {
            if (coalesced.Count > 0 &&
                item.Y - coalesced[^1].Y < 4)
            {
                coalesced[^1] = (
                item.Marker.IsComment
                    ? item.Marker
                    : coalesced[^1].Marker,
                (coalesced[^1].Y + item.Y) / 2);
                continue;
            }

            coalesced.Add((item.Marker, item.Y));
        }

        foreach (var (marker, y) in coalesced)
        {
            var tick = new Border
            {
                Width = 8,
                Height = 4,
                CornerRadius = new CornerRadius(2),
                Background = Application.Current.Resources[
                marker.IsComment
                    ? "AccentTextFillColorPrimaryBrush"
                    : marker.IsAddition
                        ? "AddedDiffLineStrokeBrush"
                        : "DeletedDiffLineStrokeBrush"] as Brush
            };
            Canvas.SetLeft(tick, 8);
            Canvas.SetTop(tick, y);
            DiffMarkerCanvas.Children.Add(tick);
            markerHitTargets.Add((y + 2, marker));
        }
    }

    private void DiffMarkerCanvas_PointerPressed(
        object sender,
        PointerRoutedEventArgs e)
    {
        if (sender is not Canvas canvas ||
            markerHitTargets.Count == 0)
        {
            return;
        }

        double y = e.GetCurrentPoint(canvas).Position.Y;
        DiffChangeMarkerViewModel nearest = markerHitTargets
            .OrderBy(item => Math.Abs(item.Y - y))
            .First()
            .Marker;
        nearest.NavigateCommand.Execute(null);
        e.Handled = true;
    }

    private void MainPage_Unloaded(object sender, RoutedEventArgs e)
    {
        ViewModel.PropertyChanged -= ViewModel_PropertyChanged;
        if (reviewRefreshTimer is not null)
        {
            reviewRefreshTimer.Tick -= ReviewRefreshTimer_Tick;
            reviewRefreshTimer.Stop();
        }

        if (subscribedReview is not null)
        {
            subscribedReview.ScrollToLineRequested -= Review_ScrollToLineRequested;
            subscribedReview.DiffMarkersChanged -= Review_DiffMarkersChanged;
            subscribedReview.CommentComposerRequested -= Review_CommentComposerRequested;
        }
    }

    private async void ReviewRefreshTimer_Tick(
        DispatcherQueueTimer sender,
        object args)
    {
        if (isRefreshingReviews)
        {
            return;
        }

        isRefreshingReviews = true;
        try
        {
            await ViewModel.RefreshOpenReviewsAsync();
        }
        finally
        {
            isRefreshingReviews = false;
        }
    }
}
