using System.Text;
using PullRequestScanner.Core.Authentication;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Repository;

public static class GitAuthentication
{
    public static string FormatAuthorizationHeader(
        ReviewProvider provider,
        ReviewCredential credential)
    {
        if (provider == ReviewProvider.GitHub &&
            credential.Scheme.Equals("Bearer", StringComparison.OrdinalIgnoreCase))
        {
            string basic = Convert.ToBase64String(
                Encoding.ASCII.GetBytes($"x-access-token:{credential.Parameter}"));
            return $"Authorization: Basic {basic}";
        }

        return $"Authorization: {credential.Scheme} {credential.Parameter}";
    }

    public static bool IndicatesAuthenticationFailure(ProcessResult result)
    {
        string error = $"{result.StandardOutput}\n{result.StandardError}"
            .ToLowerInvariant();
        return error.Contains("invalid credentials", StringComparison.Ordinal) ||
               error.Contains("authentication failed", StringComparison.Ordinal) ||
               error.Contains("could not read username", StringComparison.Ordinal) ||
               error.Contains("could not read password", StringComparison.Ordinal) ||
               error.Contains("terminal prompts disabled", StringComparison.Ordinal) ||
               error.Contains("authentication required", StringComparison.Ordinal) ||
               error.Contains("http 401", StringComparison.Ordinal) ||
               error.Contains("http 403", StringComparison.Ordinal) ||
               error.Contains("repository not found", StringComparison.Ordinal);
    }
}
