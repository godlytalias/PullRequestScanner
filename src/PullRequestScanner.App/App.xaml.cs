using PullRequestScanner.App.Services;
using Microsoft.UI.Xaml;

namespace PullRequestScanner.App;

public partial class App : Application
{
    private Window? _window;

    public static AppServiceFactory Services { get; private set; } = null!;
    public static Window? MainWindow { get; private set; }

    public App()
    {
        InitializeComponent();
        HighContrastAdjustment = ApplicationHighContrastAdjustment.None;
    }

    protected override void OnLaunched(Microsoft.UI.Xaml.LaunchActivatedEventArgs args)
    {
        string localDataRoot = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "PullRequestScanner");
        Services = AppServiceFactory.Create(localDataRoot);
        _window = new MainWindow();
        MainWindow = _window;
        _window.Closed += (_, _) =>
        {
            MainWindow = null;
            Services.Dispose();
        };
        _window.Activate();
    }
}
