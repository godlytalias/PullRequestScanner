using PullRequestScanner.Core;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Tests;

public sealed class PullRequestUrlParserTests
{
    [Fact]
    public void ParsesGitHubPullRequest()
    {
        PullRequestReference reference = PullRequestUrlParser.Parse(
            "https://github.com/octo-org/code-review-demo/pull/12345/files");

        Assert.Equal(ReviewProvider.GitHub, reference.Provider);
        Assert.Equal("octo-org", reference.Organization);
        Assert.Equal("code-review-demo", reference.Repository);
        Assert.Equal(12345, reference.Number);
        Assert.Equal("https://api.github.com", reference.ApiBaseUrl);
        Assert.Equal(
            "https://github.com/octo-org/code-review-demo.git",
            reference.CloneUrl);
    }

    [Fact]
    public void ParsesGitHubEnterprisePullRequest()
    {
        PullRequestReference reference = PullRequestUrlParser.Parse(
            "https://git.contoso.test/contoso/runtime/pull/42");

        Assert.Equal(ReviewProvider.GitHub, reference.Provider);
        Assert.Equal("git.contoso.test", reference.Host);
        Assert.Equal("https://git.contoso.test/api/v3", reference.ApiBaseUrl);
        Assert.Equal(42, reference.Number);
    }

    [Fact]
    public void PreservesGitHubEnterpriseNonDefaultPort()
    {
        PullRequestReference reference = PullRequestUrlParser.Parse(
            "http://127.0.0.1:8123/owner/repo/pull/1");

        Assert.Equal("127.0.0.1:8123", reference.Host);
        Assert.Equal("http://127.0.0.1:8123/api/v3", reference.ApiBaseUrl);
        Assert.Equal("http://127.0.0.1:8123/owner/repo.git", reference.CloneUrl);
    }

    [Fact]
    public void ParsesAzureDevOpsPullRequest()
    {
        PullRequestReference reference = PullRequestUrlParser.Parse(
            "https://dev.azure.com/contoso/Project/_git/repository/pullrequest/98765");

        Assert.Equal(ReviewProvider.AzureDevOps, reference.Provider);
        Assert.Equal("contoso", reference.Organization);
        Assert.Equal("Project", reference.Project);
        Assert.Equal("repository", reference.Repository);
        Assert.Equal(98765, reference.Number);
        Assert.Equal("https://dev.azure.com/contoso/Project", reference.ApiBaseUrl);
    }

    [Fact]
    public void ParsesLegacyVisualStudioUrl()
    {
        PullRequestReference reference = PullRequestUrlParser.Parse(
            "https://contoso.visualstudio.com/Project/_git/repository/pullrequest/8");

        Assert.Equal(ReviewProvider.AzureDevOps, reference.Provider);
        Assert.Equal("contoso", reference.Organization);
        Assert.Equal("Project", reference.Project);
        Assert.Equal("repository", reference.Repository);
    }

    [Theory]
    [InlineData("")]
    [InlineData("github.com/octo-org/repo/pull/1")]
    [InlineData("https://github.com/octo-org/repo/issues/1")]
    [InlineData("https://dev.azure.com/contoso/Project/_git/repository/pullrequest/not-a-number")]
    public void RejectsUnsupportedUrls(string url)
    {
        bool parsed = PullRequestUrlParser.TryParse(
            url,
            out _,
            out string? error);

        Assert.False(parsed);
        Assert.False(string.IsNullOrWhiteSpace(error));
    }
}
