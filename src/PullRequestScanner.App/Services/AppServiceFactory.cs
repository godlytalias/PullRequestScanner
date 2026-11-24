using PullRequestScanner.Core;
using PullRequestScanner.Core.Authentication;
using PullRequestScanner.Core.Diff;
using PullRequestScanner.Core.Navigation;
using PullRequestScanner.Core.Persistence;
using PullRequestScanner.Core.Providers;
using PullRequestScanner.Core.Repository;
using PullRequestScanner.Core.Services;

namespace PullRequestScanner.App.Services;

public sealed class AppServiceFactory : IDisposable
{
    private readonly HttpClient httpClient;

    private AppServiceFactory(
        ReviewSessionService reviewSessions,
        ReviewActivityService reviewActivity,
        HttpClient httpClient)
    {
        ReviewSessions = reviewSessions;
        ReviewActivity = reviewActivity;
        this.httpClient = httpClient;
    }

    public ReviewSessionService ReviewSessions { get; }
    public ReviewActivityService ReviewActivity { get; }

    public static AppServiceFactory Create(string localDataRoot)
    {
        var processRunner = new ProcessRunner();
        var credentialProvider = new CliCredentialProvider(processRunner);
        var httpClient = new HttpClient
        {
            Timeout = TimeSpan.FromMinutes(5)
        };
        var providerHttp = new ReviewProviderHttp(httpClient, credentialProvider);
        var providerFactory = new ReviewProviderFactory(
            new GitHubReviewProviderClient(providerHttp),
            new AzureDevOpsReviewProviderClient(providerHttp));

        string userProfile = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        var repositoryRoots = new List<string>
        {
            Path.Combine(userProfile, "repos"),
            Path.Combine(userProfile, "src")
        };
        string? configuredRoots = Environment.GetEnvironmentVariable("PULLREQUEST_SCANNER_REPO_ROOTS");
        if (!string.IsNullOrWhiteSpace(configuredRoots))
        {
            repositoryRoots.AddRange(
                configuredRoots.Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries));
        }

        var stateStore = new JsonReviewStateStore(Path.Combine(localDataRoot, "Reviews"));
        var activityStore = new JsonReviewActivityStore(
            Path.Combine(localDataRoot, "Activity", "review-activity.json"));
        var reviewActivity = new ReviewActivityService(activityStore);
        var repositoryService = new LocalRepositoryService(
            processRunner,
            credentialProvider,
            Path.Combine(localDataRoot, "Repositories"),
            repositoryRoots);
        var diffService = new GitDiffService(processRunner);
        var symbolService = new SymbolNavigationService(processRunner);
        var reviewSessions = new ReviewSessionService(
            providerFactory,
            repositoryService,
            diffService,
            symbolService,
            stateStore,
            credentialProvider,
            reviewActivity);
        return new AppServiceFactory(reviewSessions, reviewActivity, httpClient);
    }

    public void Dispose() => httpClient.Dispose();
}
