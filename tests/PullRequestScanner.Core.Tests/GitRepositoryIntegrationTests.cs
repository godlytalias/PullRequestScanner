using PullRequestScanner.Core.Diff;
using PullRequestScanner.Core.Models;
using PullRequestScanner.Core.Navigation;

namespace PullRequestScanner.Core.Tests;

public sealed class GitRepositoryIntegrationTests
{
    [Fact]
    public async Task ReadsChangedFilesDiffAndDefinitionFromLocalGitObjects()
    {
        string root = Path.Combine(
            Path.GetTempPath(),
            "PullRequestScanner.Tests",
            Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(Path.Combine(root, "src"));
        var runner = new ProcessRunner();

        try
        {
            await RunGitAsync(runner, root, "init", "--quiet");
            await RunGitAsync(runner, root, "config", "core.autocrlf", "false");
            await RunGitAsync(runner, root, "config", "core.safecrlf", "false");
            await RunGitAsync(runner, root, "config", "user.email", "tests@example.invalid");
            await RunGitAsync(runner, root, "config", "user.name", "PullRequest Scanner Tests");
            await File.WriteAllTextAsync(
                Path.Combine(root, "src", "MathCore.cs"),
                """
                public sealed class MathCore
                {
                    public static int Calculate(int value) => value * 2;
                }
                """);
            await File.WriteAllTextAsync(
                Path.Combine(root, "src", "Consumer.cs"),
                """
                public sealed class Consumer
                {
                    public int Read() => 1;
                }
                """);
            await RunGitAsync(runner, root, "add", ".");
            await RunGitAsync(runner, root, "commit", "--quiet", "-m", "base");
            string baseCommit = (await RunGitAsync(
                runner,
                root,
                "rev-parse",
                "HEAD")).StandardOutput.Trim();

            await File.WriteAllTextAsync(
                Path.Combine(root, "src", "Consumer.cs"),
                """
                public sealed class Consumer
                {
                    public int Read(int value) => MathCore.Calculate(value);
                }
                """);
            await RunGitAsync(runner, root, "add", "src/Consumer.cs");
            await RunGitAsync(runner, root, "commit", "--quiet", "-m", "review");
            string headCommit = (await RunGitAsync(
                runner,
                root,
                "rev-parse",
                "HEAD")).StandardOutput.Trim();

            var state = new ReviewSessionState
            {
                LocalRepositoryPath = root,
                Details = new PullRequestDetails
                {
                    BaseCommitSha = baseCommit,
                    HeadCommitSha = headCommit,
                    Reference = new PullRequestReference
                    {
                        Provider = ReviewProvider.GitHub,
                        Repository = "test"
                    }
                }
            };

            var diffService = new GitDiffService(runner);
            ChangedFile changed = Assert.Single(
                await diffService.ListChangedFilesAsync(state));
            Assert.Equal("src/Consumer.cs", changed.Path);
            IReadOnlyList<DiffBlock> diff = await diffService.GetDiffAsync(
                state,
                changed.Path,
                contextLines: 3,
                completeFile: false);
            DiffBlock block = Assert.Single(diff);
            Assert.Contains(block.Lines, line =>
                line.Kind == DiffLineKind.Added &&
                line.Text.Contains("MathCore.Calculate", StringComparison.Ordinal));

            string source = await diffService.ReadFileAsync(
                state,
                headCommit,
                "src/Consumer.cs");
            Assert.Contains("Calculate", source, StringComparison.Ordinal);

            SymbolLocation? definition = await new SymbolNavigationService(runner)
                .FindDefinitionAsync(state, "Calculate", "src/Consumer.cs");
            Assert.NotNull(definition);
            Assert.Equal("src/MathCore.cs", definition.FilePath);
            Assert.Contains("static int Calculate", definition.LineText, StringComparison.Ordinal);
        }
        finally
        {
            if (Directory.Exists(root))
            {
                RemoveDirectory(root);
            }
        }
    }

    [Fact]
    public async Task DefinitionNavigationRejectsCallSiteOnlyMatches()
    {
        await WithRepositoryAsync(
            """
            #include <algorithm>

            int Read(int value)
            {
                return std::clamp(value, 0, 10);
            }
            """,
            async (runner, root, state) =>
            {
                SymbolLocation? definition = await new SymbolNavigationService(runner)
                    .FindDefinitionAsync(
                        state,
                        "clamp",
                        "src/revealtestapi.cpp",
                        state.Details.HeadCommitSha,
                        sourceLineNumber: 5);

                Assert.Null(definition);
            });
    }

    [Fact]
    public async Task DefinitionNavigationFindsDeclarationAfterEarlierCalls()
    {
        string calls = string.Join(
            "\n",
            Enumerable.Range(1, 300).Select(index => $"    values[{index - 1}] = clamp({index});"));
        string source = $$"""
            #include <array>

            int Run()
            {
            {{calls}}
                return 0;
            }

            int clamp(int value)
            {
                return value;
            }
            """;

        await WithRepositoryAsync(
            source,
            async (runner, root, state) =>
            {
                SymbolLocation? definition = await new SymbolNavigationService(runner)
                    .FindDefinitionAsync(
                        state,
                        "clamp",
                        "src/revealtestapi.cpp",
                        state.Details.HeadCommitSha,
                        sourceLineNumber: 6);

                Assert.NotNull(definition);
                Assert.Equal("src/revealtestapi.cpp", definition.FilePath);
                Assert.Contains("int clamp", definition.LineText, StringComparison.Ordinal);
            });
    }

    private static async Task<ProcessResult> RunGitAsync(
        ProcessRunner runner,
        string root,
        params string[] arguments)
    {
        ProcessResult result = await runner.RunAsync(
            "git",
            ["-C", root, .. arguments]);
        Assert.True(
            result.Succeeded,
            $"git {string.Join(' ', arguments)} failed: {result.StandardError}");
        return result;
    }

    private static async Task WithRepositoryAsync(
        string source,
        Func<ProcessRunner, string, ReviewSessionState, Task> action)
    {
        string root = Path.Combine(
            Path.GetTempPath(),
            "PullRequestScanner.Tests",
            Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(Path.Combine(root, "src"));
        var runner = new ProcessRunner();

        try
        {
            await RunGitAsync(runner, root, "init", "--quiet");
            await RunGitAsync(runner, root, "config", "core.autocrlf", "false");
            await RunGitAsync(runner, root, "config", "core.safecrlf", "false");
            await RunGitAsync(runner, root, "config", "user.email", "tests@example.invalid");
            await RunGitAsync(runner, root, "config", "user.name", "PullRequest Scanner Tests");
            await File.WriteAllTextAsync(
                Path.Combine(root, "src", "revealtestapi.cpp"),
                source);
            await RunGitAsync(runner, root, "add", ".");
            await RunGitAsync(runner, root, "commit", "--quiet", "-m", "fixture");
            string commit = (await RunGitAsync(
                runner,
                root,
                "rev-parse",
                "HEAD")).StandardOutput.Trim();
            var state = new ReviewSessionState
            {
                LocalRepositoryPath = root,
                Details = new PullRequestDetails
                {
                    BaseCommitSha = commit,
                    HeadCommitSha = commit,
                    Reference = new PullRequestReference
                    {
                        Provider = ReviewProvider.GitHub,
                        Repository = "test"
                    }
                }
            };

            await action(runner, root, state);
        }
        finally
        {
            if (Directory.Exists(root))
            {
                RemoveDirectory(root);
            }
        }
    }

    private static void RemoveDirectory(string root)
    {
        var directory = new DirectoryInfo(root);
        foreach (FileInfo file in directory.EnumerateFiles("*", SearchOption.AllDirectories))
        {
            file.Attributes = FileAttributes.Normal;
        }

        foreach (DirectoryInfo child in directory
                     .EnumerateDirectories("*", SearchOption.AllDirectories)
                     .Reverse())
        {
            child.Attributes = FileAttributes.Normal;
        }

        directory.Attributes = FileAttributes.Normal;
        directory.Delete(recursive: true);
    }
}
