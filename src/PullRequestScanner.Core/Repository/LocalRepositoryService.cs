using System.ComponentModel;
using System.Security.Cryptography;
using System.Text;
using PullRequestScanner.Core.Authentication;
using PullRequestScanner.Core.Exceptions;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Repository;

public sealed record LocalRepository(string Path, bool ReusedExistingCheckout);

public sealed class LocalRepositoryService
{
    private readonly ProcessRunner processRunner;
    private readonly ICredentialProvider credentialProvider;
    private readonly string cacheRoot;
    private readonly IReadOnlyList<string> searchRoots;

    public LocalRepositoryService(
        ProcessRunner processRunner,
        ICredentialProvider credentialProvider,
        string cacheRoot,
        IEnumerable<string>? searchRoots = null)
    {
        this.processRunner = processRunner;
        this.credentialProvider = credentialProvider;
        this.cacheRoot = Path.GetFullPath(cacheRoot);
        searchRoots ??=
        [
            Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
                "repos"),
            Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
                "src")
        ];
        this.searchRoots = searchRoots
            .Where(path => !string.IsNullOrWhiteSpace(path))
            .Select(Path.GetFullPath)
            .Distinct(StringComparer.OrdinalIgnoreCase)
            .ToArray();
    }

    public async Task<LocalRepository> PrepareAsync(
        PullRequestDetails pullRequest,
        string? preferredPath = null,
        CancellationToken cancellationToken = default)
    {
        try
        {
            IReadOnlyList<IReadOnlyDictionary<string, string>> gitEnvironments =
                await CreateGitEnvironmentsAsync(
                pullRequest.Reference,
                cancellationToken);
            if (!string.IsNullOrWhiteSpace(preferredPath))
            {
                string preferredRoot = await GetRepositoryRootAsync(preferredPath, cancellationToken);
                await EnsureMatchingRemoteAsync(preferredRoot, pullRequest, cancellationToken);
                await FetchReviewCommitsAsync(
                    preferredRoot,
                    pullRequest,
                    gitEnvironments,
                    cancellationToken);
                return new LocalRepository(preferredRoot, ReusedExistingCheckout: true);
            }

            string cachePath = GetCachePath(pullRequest);
            LocalRepository? existing = await FindMatchingCheckoutAsync(pullRequest, cachePath, cancellationToken);
            if (existing is not null)
            {
                await FetchReviewCommitsAsync(
                    existing.Path,
                    pullRequest,
                    gitEnvironments,
                    cancellationToken);
                return existing;
            }

            Directory.CreateDirectory(Path.GetDirectoryName(cachePath)!);
            string target = cachePath;
            await CloneAsync(target, pullRequest, gitEnvironments, cancellationToken);

            await FetchReviewCommitsAsync(
                target,
                pullRequest,
                gitEnvironments,
                cancellationToken);
            return new LocalRepository(Path.GetFullPath(target), ReusedExistingCheckout: false);
        }
        catch (Win32Exception exception)
        {
            throw new ReviewOperationException(
                "Git is required to prepare the repository, but the git executable was not found on PATH.",
                exception);
        }
    }

    private async Task<LocalRepository?> FindMatchingCheckoutAsync(
        PullRequestDetails pullRequest,
        string cachePath,
        CancellationToken cancellationToken)
    {
        if (Directory.Exists(cachePath) &&
            await TryRemoteMatchesAsync(cachePath, pullRequest.CloneUrl, cancellationToken))
        {
            return new LocalRepository(Path.GetFullPath(cachePath), ReusedExistingCheckout: false);
        }

        foreach (string searchRoot in searchRoots)
        {
            if (!Directory.Exists(searchRoot))
            {
                continue;
            }

            foreach (string candidate in Directory.EnumerateDirectories(searchRoot, "*", SearchOption.TopDirectoryOnly))
            {
                if (await TryRemoteMatchesAsync(candidate, pullRequest.CloneUrl, cancellationToken))
                {
                    return new LocalRepository(Path.GetFullPath(candidate), ReusedExistingCheckout: true);
                }
            }
        }

        return null;
    }

    private async Task<string> GetRepositoryRootAsync(
        string path,
        CancellationToken cancellationToken)
    {
        string fullPath = Path.GetFullPath(Environment.ExpandEnvironmentVariables(path));
        if (!Directory.Exists(fullPath))
        {
            throw new ReviewOperationException($"The selected repository path does not exist: {fullPath}");
        }

        ProcessResult result = await processRunner.RunAsync(
            "git",
            ["-C", fullPath, "rev-parse", "--show-toplevel"],
            cancellationToken: cancellationToken);
        if (!result.Succeeded)
        {
            throw new ReviewOperationException(
                $"The selected path is not a Git checkout: {fullPath}");
        }

        return Path.GetFullPath(result.StandardOutput.Trim());
    }

    private async Task EnsureMatchingRemoteAsync(
        string repositoryPath,
        PullRequestDetails pullRequest,
        CancellationToken cancellationToken)
    {
        if (!await TryRemoteMatchesAsync(repositoryPath, pullRequest.CloneUrl, cancellationToken))
        {
            ProcessResult remote = await processRunner.RunAsync(
                "git",
                ["-C", repositoryPath, "remote", "get-url", "origin"],
                cancellationToken: cancellationToken);
            throw new ReviewOperationException(
                $"The selected checkout '{repositoryPath}' does not use the pull request repository " +
                $"({remote.StandardOutput.Trim() ?? "origin is not configured"} vs. {pullRequest.CloneUrl}).");
        }
    }

    private async Task<bool> TryRemoteMatchesAsync(
        string repositoryPath,
        string expectedUrl,
        CancellationToken cancellationToken)
    {
        ProcessResult result = await processRunner.RunAsync(
            "git",
            ["-C", repositoryPath, "remote", "get-url", "origin"],
            cancellationToken: cancellationToken);
        return result.Succeeded &&
               RemoteUrlNormalizer.Normalize(result.StandardOutput) ==
               RemoteUrlNormalizer.Normalize(expectedUrl);
    }

    private async Task FetchReviewCommitsAsync(
        string repositoryPath,
        PullRequestDetails pullRequest,
        IReadOnlyList<IReadOnlyDictionary<string, string>> gitEnvironments,
        CancellationToken cancellationToken)
    {
        string refNamespace = CreateHash(pullRequest.Reference.SessionKey);
        string headRef = $"refs/pullrequest-scanner/review/{refNamespace}/head";
        string baseRef = $"refs/pullrequest-scanner/review/{refNamespace}/base";
        var failures = new List<ProcessResult>();
        bool succeeded = false;
        foreach (IReadOnlyDictionary<string, string> gitEnvironment in gitEnvironments)
        {
            ProcessResult fetch = await processRunner.RunAsync(
                "git",
                [
                    "-C",
                    repositoryPath,
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "origin",
                    $"+{pullRequest.SourceFetchRef}:{headRef}",
                    $"+{pullRequest.TargetFetchRef}:{baseRef}"
                ],
                environment: gitEnvironment,
                cancellationToken: cancellationToken);
            if (fetch.Succeeded)
            {
                succeeded = true;
                break;
            }

            failures.Add(fetch);
            if (GitAuthentication.IndicatesAuthenticationFailure(fetch))
            {
                continue;
            }

            ProcessResult byCommit = await processRunner.RunAsync(
                "git",
                [
                    "-C",
                    repositoryPath,
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "origin",
                    pullRequest.HeadCommitSha,
                    pullRequest.BaseCommitSha
                ],
                environment: gitEnvironment,
                cancellationToken: cancellationToken);
            failures.Add(byCommit);
            if (byCommit.Succeeded)
            {
                succeeded = true;
                break;
            }
        }

        if (!succeeded)
        {
            ThrowFetchFailure(pullRequest.Reference, failures);
        }

        await EnsureCommitExistsAsync(repositoryPath, pullRequest.BaseCommitSha, cancellationToken);
        await EnsureCommitExistsAsync(repositoryPath, pullRequest.HeadCommitSha, cancellationToken);
    }

    private async Task EnsureCommitExistsAsync(
        string repositoryPath,
        string commit,
        CancellationToken cancellationToken)
    {
        ProcessResult result = await processRunner.RunAsync(
            "git",
            ["-C", repositoryPath, "cat-file", "-e", $"{commit}^{{commit}}"],
            cancellationToken: cancellationToken);
        if (!result.Succeeded)
        {
            throw new ReviewOperationException($"Commit {commit} was not available after fetching the repository.");
        }
    }

    private string GetCachePath(PullRequestDetails pullRequest) =>
        Path.Combine(cacheRoot, CreateHash(RemoteUrlNormalizer.Normalize(pullRequest.CloneUrl)));

    public async Task DeleteCachedRepositoryAsync(
        string? repositoryPath,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(repositoryPath))
        {
            return;
        }

        string path = Path.GetFullPath(repositoryPath);
        if (!IsManagedCachePath(path))
        {
            throw new ReviewOperationException(
                "Refusing to delete a repository outside the managed PullRequest Scanner cache.");
        }

        if (!Directory.Exists(path))
        {
            return;
        }

        await Task.Run(
            () =>
            {
                SetWritable(new DirectoryInfo(path));
                Directory.Delete(path, recursive: true);
            },
            cancellationToken);
    }

    public bool IsManagedCachePath(string? repositoryPath)
    {
        if (string.IsNullOrWhiteSpace(repositoryPath))
        {
            return false;
        }

        string path = Path.GetFullPath(repositoryPath);
        string relative = Path.GetRelativePath(cacheRoot, path);
        return !relative.StartsWith("..", StringComparison.Ordinal) &&
            !Path.IsPathRooted(relative) &&
            relative.Equals(
                Path.GetFileName(path),
                StringComparison.OrdinalIgnoreCase);
    }

    private async Task<IReadOnlyList<IReadOnlyDictionary<string, string>>> CreateGitEnvironmentsAsync(
        PullRequestReference reference,
        CancellationToken cancellationToken)
    {
        var environment = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
        {
            ["GIT_TERMINAL_PROMPT"] = "0"
        };
        ReviewCredential? credential = await credentialProvider.GetCredentialAsync(
            reference,
            cancellationToken);
        if (credential is not null)
        {
            Uri cloneUri = new(reference.CloneUrl, UriKind.Absolute);
            environment["GIT_CONFIG_COUNT"] = "1";
            environment["GIT_CONFIG_KEY_0"] =
                $"http.{cloneUri.Scheme}://{cloneUri.Authority}/.extraHeader";
            environment["GIT_CONFIG_VALUE_0"] =
                GitAuthentication.FormatAuthorizationHeader(
                    reference.Provider,
                    credential);

            return [environment];
        }

        return [environment];
    }

    private async Task CloneAsync(
        string target,
        PullRequestDetails pullRequest,
        IReadOnlyList<IReadOnlyDictionary<string, string>> gitEnvironments,
        CancellationToken cancellationToken)
    {
        var failures = new List<ProcessResult>();
        foreach (IReadOnlyDictionary<string, string> gitEnvironment in gitEnvironments)
        {
            string[][] argumentSets =
            [
                [
                    "clone",
                    "--filter=blob:none",
                    "--no-checkout",
                    "--quiet",
                    pullRequest.CloneUrl,
                    target
                ],
                [
                    "clone",
                    "--no-checkout",
                    "--quiet",
                    pullRequest.CloneUrl,
                    target
                ]
            ];
            foreach (string[] arguments in argumentSets)
            {
                if (Directory.Exists(target))
                {
                    Directory.Delete(target, recursive: true);
                }

                ProcessResult clone = await processRunner.RunAsync(
                    "git",
                    arguments,
                    environment: gitEnvironment,
                    cancellationToken: cancellationToken);
                if (clone.Succeeded)
                {
                    return;
                }

                failures.Add(clone);
                if (GitAuthentication.IndicatesAuthenticationFailure(clone))
                {
                    break;
                }
            }
        }

        if (failures.Any(GitAuthentication.IndicatesAuthenticationFailure))
        {
            throw new ReviewAuthenticationException(
                pullRequest.Reference,
                $"{pullRequest.Reference.Provider} Git access requires sign-in. " +
                "Open the sign-in flow and retry the pull request.");
        }

        string details = string.Join(
            Environment.NewLine,
            failures.Select(FormatError).Distinct(StringComparer.Ordinal));
        throw new ReviewOperationException(
            $"Git could not clone {pullRequest.CloneUrl}. {details}");
    }

    private static void ThrowFetchFailure(
        PullRequestReference reference,
        IReadOnlyList<ProcessResult> failures)
    {
        if (failures.Any(GitAuthentication.IndicatesAuthenticationFailure))
        {
            throw new ReviewAuthenticationException(
                reference,
                $"{reference.Provider} Git access requires sign-in. " +
                "Open the sign-in flow and retry the pull request.");
        }

        string details = string.Join(
            Environment.NewLine,
            failures.Select(FormatError).Distinct(StringComparer.Ordinal));
        throw new ReviewOperationException(
            $"Git could not fetch the pull request commits. {details}");
    }

    private static string CreateHash(string value) =>
        Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(value)))[..20].ToLowerInvariant();

    private static string FormatError(ProcessResult result)
    {
        string error = result.StandardError.Trim();
        return string.IsNullOrWhiteSpace(error) ? $"git exited with code {result.ExitCode}." : error;
    }

    private static void SetWritable(DirectoryInfo directory)
    {
        foreach (FileInfo file in directory.EnumerateFiles(
                     "*",
                     SearchOption.AllDirectories))
        {
            file.Attributes = FileAttributes.Normal;
        }

        foreach (DirectoryInfo child in directory
                     .EnumerateDirectories("*", SearchOption.AllDirectories)
                     .OrderByDescending(item => item.FullName.Length))
        {
            child.Attributes = FileAttributes.Normal;
        }

        directory.Attributes = FileAttributes.Normal;
    }

}
