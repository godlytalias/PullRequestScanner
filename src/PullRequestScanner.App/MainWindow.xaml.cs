using Microsoft.UI.Xaml;
using Microsoft.UI.Windowing;

namespace PullRequestScanner.App;

public sealed partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();

        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);

        AppWindow.SetIcon("Assets/AppIcon.ico");
        if (AppWindow.Presenter is OverlappedPresenter presenter)
        {
            presenter.Maximize();
        }
        RootFrame.Navigate(typeof(MainPage));
    }

    private void OpenPullRequestTitleButton_Click(
        object sender,
        RoutedEventArgs e)
    {
        if (RootFrame.Content is MainPage page)
        {
            page.ShowOpenReviewPanel();
        }
    }
}
