using System.Collections.Concurrent;
using System.ComponentModel;
using System.Diagnostics;
using System.Text;
using PullRequestScanner.Core.Exceptions;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Authentication;

public sealed record ReviewCredential(string Scheme, string Parameter);

public interface ICredentialProvider
{
    Task<ReviewCredential?> GetCredentialAsync(
        PullRequestReference reference,
        CancellationToken cancellationToken = default);

    Task AuthenticateAsync(
        PullRequestReference reference,
        CancellationToken cancellationToken = default) =>
        throw new NotSupportedException("Interactive authentication is not available.");
}

public sealed class CliCredentialProvider(ProcessRunner processRunner) : ICredentialProvider
{
    private const string AzureDevOpsResource = "499b84ac-1321-427f-aa17-267ca6975798";
    private readonly ConcurrentDictionary<ReviewProvider, byte> ignoredEnvironmentCredentials = new();

    public async Task<ReviewCredential?> GetCredentialAsync(
        PullRequestReference reference,
        CancellationToken cancellationToken = default) =>
        reference.Provider switch
        {
            ReviewProvider.GitHub => await GetGitHubCredentialAsync(reference, cancellationToken),
            ReviewProvider.AzureDevOps => await GetAzureDevOpsCredentialAsync(cancellationToken),
            _ => null
        };

    private async Task<ReviewCredential?> GetGitHubCredentialAsync(
        PullRequestReference reference,
        CancellationToken cancellationToken)
    {
        string? token = ignoredEnvironmentCredentials.ContainsKey(ReviewProvider.GitHub)
            ? null
            : Environment.GetEnvironmentVariable("GH_TOKEN")
                ?? Environment.GetEnvironmentVariable("GITHUB_TOKEN")
                ?? Environment.GetEnvironmentVariable("PULLREQUEST_SCANNER_GITHUB_TOKEN");
        if (!string.IsNullOrWhiteSpace(token))
        {
            return new ReviewCredential("Bearer", token.Trim());
        }

        ProcessResult result = await TryRunAsync(
            "gh",
            ["auth", "token", "--hostname", reference.Host],
            cancellationToken);
        string value = result.StandardOutput.Trim();
        return result.Succeeded && !string.IsNullOrWhiteSpace(value)
            ? new ReviewCredential("Bearer", value)
            : null;
    }

    private async Task<ReviewCredential?> GetAzureDevOpsCredentialAsync(CancellationToken cancellationToken)
    {
        string? pat = ignoredEnvironmentCredentials.ContainsKey(ReviewProvider.AzureDevOps)
            ? null
            : Environment.GetEnvironmentVariable("AZURE_DEVOPS_EXT_PAT")
                ?? Environment.GetEnvironmentVariable("PULLREQUEST_SCANNER_AZURE_DEVOPS_PAT")
                ?? Environment.GetEnvironmentVariable("SYSTEM_ACCESSTOKEN");
        if (!string.IsNullOrWhiteSpace(pat))
        {
            string parameter = Convert.ToBase64String(Encoding.ASCII.GetBytes($":{pat.Trim()}"));
            return new ReviewCredential("Basic", parameter);
        }

        ProcessResult result = await TryRunAsync(
            "az",
            [
                "account",
                "get-access-token",
                "--resource",
                AzureDevOpsResource,
                "--query",
                "accessToken",
                "--output",
                "tsv",
                "--only-show-errors"
            ],
            cancellationToken);
        string value = result.StandardOutput.Trim();
        return result.Succeeded && !string.IsNullOrWhiteSpace(value)
            ? new ReviewCredential("Bearer", value)
            : null;
    }

    private async Task<ProcessResult> TryRunAsync(
        string fileName,
        IReadOnlyList<string> arguments,
        CancellationToken cancellationToken)
    {
        try
        {
            return await processRunner.RunAsync(
                fileName,
                arguments,
                cancellationToken: cancellationToken);
        }
        catch (Win32Exception)
        {
            return new ProcessResult(127, string.Empty, $"{fileName} is not installed or is not on PATH.");
        }
    }

    public async Task AuthenticateAsync(
        PullRequestReference reference,
        CancellationToken cancellationToken = default)
    {
        (string fileName, string[] arguments) = GetAuthenticationCommand(reference);
        var startInfo = new ProcessStartInfo
        {
            FileName = fileName,
            UseShellExecute = false,
            CreateNoWindow = false
        };
        foreach (string argument in arguments)
        {
            startInfo.ArgumentList.Add(argument);
        }
        ClearProviderTokenEnvironment(reference.Provider, startInfo);

        try
        {
            using Process process = Process.Start(startInfo) ??
                throw new ReviewOperationException(
                    $"Could not start the {reference.Provider} sign-in command.");
            await process.WaitForExitAsync(cancellationToken);
            if (process.ExitCode != 0)
            {
                throw new ReviewOperationException(
                    $"{reference.Provider} sign-in did not complete successfully (exit {process.ExitCode}).");
            }
        }
        catch (Win32Exception exception)
        {
            string tool = reference.Provider == ReviewProvider.GitHub ? "gh" : "az";
            throw new ReviewOperationException(
                $"{tool} is required for interactive {reference.Provider} sign-in but was not found on PATH.",
                exception);
        }

        ignoredEnvironmentCredentials.TryAdd(reference.Provider, 0);
        ReviewCredential? credential = await GetCredentialAsync(
            reference,
            cancellationToken);
        if (credential is null)
        {
            throw new ReviewOperationException(
                $"{reference.Provider} sign-in completed, but no usable credential was found. Close the sign-in window only after authentication succeeds, then retry.");
        }
    }

    private static void ClearProviderTokenEnvironment(
        ReviewProvider provider,
        ProcessStartInfo startInfo)
    {
        string[] names = provider == ReviewProvider.GitHub
            ? ["GH_TOKEN", "GITHUB_TOKEN", "PULLREQUEST_SCANNER_GITHUB_TOKEN"]
            : [
                "AZURE_DEVOPS_EXT_PAT",
                "PULLREQUEST_SCANNER_AZURE_DEVOPS_PAT",
                "SYSTEM_ACCESSTOKEN"
            ];
        foreach (string name in names)
        {
            startInfo.Environment.Remove(name);
        }
    }

    private static (string FileName, string[] Arguments) GetAuthenticationCommand(
        PullRequestReference reference) =>
        reference.Provider switch
        {
            ReviewProvider.GitHub =>
                ("gh",
                [
                    "auth",
                    "login",
                    "--hostname",
                    reference.Host,
                    "--git-protocol",
                    "https",
                    "--web",
                    "--skip-ssh-key"
                ]),
            ReviewProvider.AzureDevOps =>
                ("az",
                [
                    "login",
                    "--use-device-code",
                    "--allow-no-subscriptions"
                ]),
            _ => throw new ArgumentOutOfRangeException(
                nameof(reference),
                reference.Provider,
                "Unsupported authentication provider.")
        };
}
