using System.ComponentModel;
using PullRequestScanner.Core.Exceptions;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Diff;

public sealed class GitDiffService(ProcessRunner processRunner)
{
    public async Task<IReadOnlyList<ChangedFile>> ListChangedFilesAsync(
        ReviewSessionState state,
        string? baseCommitSha = null,
        string? headCommitSha = null,
        CancellationToken cancellationToken = default)
    {
        string repositoryPath = RequireRepository(state);
        ProcessResult result = await RunGitAsync(
            repositoryPath,
            [
                "diff",
                "--name-status",
                "-z",
                "--find-renames=50%",
                baseCommitSha ?? state.Details.BaseCommitSha,
                headCommitSha ?? state.Details.HeadCommitSha
            ],
            cancellationToken);
        EnsureSucceeded(result, "list pull request files");

        return ParseNameStatus(result.StandardOutput);
    }

    public async Task<IReadOnlyList<DiffBlock>> GetDiffAsync(
        ReviewSessionState state,
        string pathSpec,
        int contextLines,
        bool completeFile,
        string? baseCommitSha = null,
        string? headCommitSha = null,
        CancellationToken cancellationToken = default)
    {
        if (contextLines < 0)
        {
            throw new ArgumentOutOfRangeException(nameof(contextLines));
        }

        string repositoryPath = RequireRepository(state);
        int context = completeFile ? 1_000_000 : contextLines;
        List<string> arguments =
        [
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--find-renames=50%",
            $"--unified={context}",
            baseCommitSha ?? state.Details.BaseCommitSha,
            headCommitSha ?? state.Details.HeadCommitSha,
            "--"
        ];

        if (!string.IsNullOrWhiteSpace(pathSpec) && pathSpec != ".")
        {
            arguments.Add(pathSpec.Replace('\\', '/'));
        }
        else
        {
            arguments.Add(".");
        }

        ProcessResult result = await RunGitAsync(repositoryPath, arguments, cancellationToken);
        EnsureSucceeded(result, "read the pull request diff");
        return UnifiedDiffParser.Parse(result.StandardOutput);
    }

    public async Task<string> ReadFileAsync(
        ReviewSessionState state,
        string commit,
        string path,
        CancellationToken cancellationToken = default)
    {
        string repositoryPath = RequireRepository(state);
        ProcessResult result = await RunGitAsync(
            repositoryPath,
            ["show", "--no-textconv", $"{commit}:{path.Replace('\\', '/')}"],
            cancellationToken);
        EnsureSucceeded(result, $"read '{path}' from commit {commit}");
        return result.StandardOutput;
    }

    private async Task<ProcessResult> RunGitAsync(
        string repositoryPath,
        IReadOnlyList<string> arguments,
        CancellationToken cancellationToken)
    {
        try
        {
            return await processRunner.RunAsync(
                "git",
                ["-C", repositoryPath, .. arguments],
                cancellationToken: cancellationToken);
        }
        catch (Win32Exception exception)
        {
            throw new ReviewOperationException(
                "Git is required to inspect the review, but the git executable was not found on PATH.",
                exception);
        }
    }

    private static IReadOnlyList<ChangedFile> ParseNameStatus(string value)
    {
        string[] fields = value.Split('\0', StringSplitOptions.None);
        var files = new List<ChangedFile>();
        for (int index = 0; index < fields.Length && !string.IsNullOrEmpty(fields[index]);)
        {
            string status = fields[index++];
            if (index >= fields.Length || string.IsNullOrEmpty(fields[index]))
            {
                break;
            }

            char code = status[0];
            if (code is 'R' or 'C')
            {
                string previous = fields[index++];
                string path = index < fields.Length ? fields[index++] : previous;
                files.Add(new ChangedFile
                {
                    Path = path,
                    PreviousPath = previous,
                    Status = code == 'R' ? ChangedFileStatus.Renamed : ChangedFileStatus.Copied
                });
            }
            else
            {
                files.Add(new ChangedFile
                {
                    Path = fields[index++],
                    Status = code switch
                    {
                        'A' => ChangedFileStatus.Added,
                        'D' => ChangedFileStatus.Deleted,
                        'T' => ChangedFileStatus.TypeChanged,
                        'U' => ChangedFileStatus.Unmerged,
                        _ => ChangedFileStatus.Modified
                    }
                });
            }
        }

        return files;
    }

    private static string RequireRepository(ReviewSessionState state)
    {
        if (string.IsNullOrWhiteSpace(state.LocalRepositoryPath) ||
            !Directory.Exists(state.LocalRepositoryPath))
        {
            throw new ReviewOperationException("The pull request repository is no longer available. Reopen the pull request to refresh it.");
        }

        return state.LocalRepositoryPath;
    }

    private static void EnsureSucceeded(ProcessResult result, string operation)
    {
        if (!result.Succeeded)
        {
            string error = result.StandardError.Trim();
            throw new ReviewOperationException(
                $"Git could not {operation} (exit {result.ExitCode}). {error}");
        }
    }
}
