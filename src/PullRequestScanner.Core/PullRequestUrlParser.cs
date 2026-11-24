using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core;

public static class PullRequestUrlParser
{
    public static PullRequestReference Parse(string value)
    {
        if (!TryParse(value, out PullRequestReference? reference, out string? error))
        {
            throw new ArgumentException(error, nameof(value));
        }

        return reference;
    }

    public static bool TryParse(
        string? value,
        out PullRequestReference reference,
        out string? error)
    {
        reference = new PullRequestReference();
        error = null;

        if (string.IsNullOrWhiteSpace(value) ||
            !Uri.TryCreate(value.Trim(), UriKind.Absolute, out Uri? uri) ||
            (uri.Scheme != Uri.UriSchemeHttps && uri.Scheme != Uri.UriSchemeHttp))
        {
            error = "Enter an absolute GitHub or Azure DevOps pull request URL.";
            return false;
        }

        string[] segments = uri.AbsolutePath
            .Split('/', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .Select(Uri.UnescapeDataString)
            .ToArray();

        if (TryParseAzureDevOps(uri, segments, out reference, out error))
        {
            return true;
        }

        if (TryParseGitHub(uri, segments, out reference, out error))
        {
            return true;
        }

        error = "The URL is not a supported GitHub or Azure DevOps pull request URL.";
        return false;
    }

    private static bool TryParseAzureDevOps(
        Uri uri,
        string[] segments,
        out PullRequestReference reference,
        out string? error)
    {
        reference = new PullRequestReference();
        error = null;

        bool isAzureHost =
            uri.Host.Equals("dev.azure.com", StringComparison.OrdinalIgnoreCase) ||
            uri.Host.EndsWith(".visualstudio.com", StringComparison.OrdinalIgnoreCase);

        int gitIndex = FindIndex(segments, "_git");
        int pullIndex = FindIndex(segments, "pullrequest");
        if (!isAzureHost || gitIndex < 1 || pullIndex != gitIndex + 2 || !TryGetNumber(segments, pullIndex + 1, out int number))
        {
            return false;
        }

        string organization;
        string project;
        string apiBaseUrl;
        if (uri.Host.Equals("dev.azure.com", StringComparison.OrdinalIgnoreCase))
        {
            if (gitIndex < 2)
            {
                return false;
            }

            organization = segments[0];
            project = string.Join('/', segments.Skip(1).Take(gitIndex - 1));
            apiBaseUrl = $"{uri.Scheme}://{uri.Host}/{Uri.EscapeDataString(organization)}/{Uri.EscapeDataString(project)}";
        }
        else
        {
            organization = uri.Host[..^".visualstudio.com".Length];
            project = string.Join('/', segments.Take(gitIndex));
            apiBaseUrl = $"{uri.Scheme}://{uri.Host}/{Uri.EscapeDataString(project)}";
        }

        string repository = segments[gitIndex + 1];
        string webUrl = BuildCleanUrl(uri);
        string cloneUrl = $"{apiBaseUrl}/_git/{Uri.EscapeDataString(repository)}";
        string key = $"ado:{organization}/{project}/{repository}#{number}".ToLowerInvariant();

        reference = new PullRequestReference
        {
            Provider = ReviewProvider.AzureDevOps,
            Host = uri.Host,
            Organization = organization,
            Project = project,
            Repository = repository,
            Number = number,
            WebUrl = webUrl,
            ApiBaseUrl = apiBaseUrl,
            CloneUrl = cloneUrl,
            SessionKey = key
        };

        return true;
    }

    private static bool TryParseGitHub(
        Uri uri,
        string[] segments,
        out PullRequestReference reference,
        out string? error)
    {
        reference = new PullRequestReference();
        error = null;

        int pullIndex = FindIndex(segments, "pull");
        if (pullIndex < 2 || pullIndex + 1 >= segments.Length || !TryGetNumber(segments, pullIndex + 1, out int number))
        {
            return false;
        }

        string owner = segments[pullIndex - 2];
        string repository = segments[pullIndex - 1];
        if (string.IsNullOrWhiteSpace(owner) || string.IsNullOrWhiteSpace(repository))
        {
            return false;
        }

        string authority = uri.IsDefaultPort ? uri.Host : uri.Authority;
        string apiBaseUrl = uri.Host.Equals("github.com", StringComparison.OrdinalIgnoreCase)
            ? "https://api.github.com"
            : $"{uri.Scheme}://{authority}/api/v3";
        string webUrl = BuildCleanUrl(uri);
        string cloneUrl = $"{uri.Scheme}://{authority}/{Uri.EscapeDataString(owner)}/{Uri.EscapeDataString(repository)}.git";
        string key = $"github:{authority}/{owner}/{repository}#{number}".ToLowerInvariant();

        reference = new PullRequestReference
        {
            Provider = ReviewProvider.GitHub,
            Host = authority,
            Organization = owner,
            Project = owner,
            Repository = repository,
            Number = number,
            WebUrl = webUrl,
            ApiBaseUrl = apiBaseUrl,
            CloneUrl = cloneUrl,
            SessionKey = key
        };

        return true;
    }

    private static int FindIndex(string[] segments, string value)
    {
        for (int index = 0; index < segments.Length; index++)
        {
            if (segments[index].Equals(value, StringComparison.OrdinalIgnoreCase))
            {
                return index;
            }
        }

        return -1;
    }

    private static bool TryGetNumber(string[] segments, int index, out int number)
    {
        number = 0;
        return index < segments.Length && int.TryParse(segments[index], out number) && number > 0;
    }

    private static string BuildCleanUrl(Uri uri) =>
        new UriBuilder(uri.Scheme, uri.Host, uri.IsDefaultPort ? -1 : uri.Port, uri.AbsolutePath).Uri.AbsoluteUri.TrimEnd('/');
}
