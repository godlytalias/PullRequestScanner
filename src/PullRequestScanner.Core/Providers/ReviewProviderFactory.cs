using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Providers;

public sealed class ReviewProviderFactory(
    GitHubReviewProviderClient gitHub,
    AzureDevOpsReviewProviderClient azureDevOps)
{
    public IReviewProviderClient Get(ReviewProvider provider) =>
        provider switch
        {
            ReviewProvider.GitHub => gitHub,
            ReviewProvider.AzureDevOps => azureDevOps,
            _ => throw new ArgumentOutOfRangeException(nameof(provider), provider, null)
        };
}
