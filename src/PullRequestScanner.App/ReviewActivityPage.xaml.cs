using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using PullRequestScanner.App.ViewModels;

namespace PullRequestScanner.App;

public sealed partial class ReviewActivityPage : Page
{
    public event EventHandler? BackRequested;

    public ReviewActivityPageViewModel ViewModel { get; } =
        new(App.Services.ReviewActivity);

    public ReviewActivityPage()
    {
        InitializeComponent();
        Loaded += ReviewActivityPage_Loaded;
    }

    public Task RefreshAsync() => ViewModel.InitializeAsync();

    private async void ReviewActivityPage_Loaded(
        object sender,
        RoutedEventArgs e)
    {
        Loaded -= ReviewActivityPage_Loaded;
        await RefreshAsync();
    }

    private void ActivityBackButton_Click(
        object sender,
        RoutedEventArgs e)
    {
        if (BackRequested is not null)
        {
            BackRequested.Invoke(this, EventArgs.Empty);
            return;
        }

        if (Frame.CanGoBack)
        {
            Frame.GoBack();
        }
        else
        {
            Frame.Navigate(typeof(MainPage));
        }
    }
}
