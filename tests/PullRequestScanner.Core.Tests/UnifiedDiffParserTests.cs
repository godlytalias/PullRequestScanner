using PullRequestScanner.Core.Diff;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Tests;

public sealed class UnifiedDiffParserTests
{
    [Fact]
    public void MapsOldAndNewLineNumbersAcrossHunk()
    {
        const string diff = """
            diff --git a/src/Foo.cs b/src/Foo.cs
            index 1111111..2222222 100644
            --- a/src/Foo.cs
            +++ b/src/Foo.cs
            @@ -10,3 +10,4 @@ class Foo
             {
            -    OldCall();
            +    NewCall();
            +    Extra();
             }
            """;

        DiffBlock block = Assert.Single(UnifiedDiffParser.Parse(diff));
        Assert.Equal("src/Foo.cs", block.FilePath);
        DiffLine deleted = Assert.Single(block.Lines, line => line.Kind == DiffLineKind.Deleted);
        Assert.Equal(11, deleted.OldLineNumber);
        Assert.Null(deleted.NewLineNumber);
        DiffLine[] added = block.Lines.Where(line => line.Kind == DiffLineKind.Added).ToArray();
        Assert.Equal([11, 12], added.Select(line => line.NewLineNumber));
        Assert.Equal(10, block.Lines[1].OldLineNumber);
        Assert.Equal(10, block.Lines[1].NewLineNumber);
    }

    [Fact]
    public void RetainsPathsForRenamedFiles()
    {
        const string diff = """
            diff --git a/src/OldName.cs b/src/NewName.cs
            similarity index 95%
            rename from src/OldName.cs
            rename to src/NewName.cs
            --- a/src/OldName.cs
            +++ b/src/NewName.cs
            @@ -1 +1 @@
            -old
            +new
            """;

        DiffBlock block = Assert.Single(UnifiedDiffParser.Parse(diff));
        Assert.Equal("src/OldName.cs", block.PreviousPath);
        Assert.Equal("src/NewName.cs", block.FilePath);
    }

    [Fact]
    public void ParsesAddedFileStartingFromZero()
    {
        const string diff = """
            diff --git a/src/New.cs b/src/New.cs
            new file mode 100644
            --- /dev/null
            +++ b/src/New.cs
            @@ -0,0 +1,2 @@
            +class New
            +{
            """;

        DiffBlock block = Assert.Single(UnifiedDiffParser.Parse(diff));
        Assert.All(block.Lines.Where(line => line.Kind == DiffLineKind.Added), line =>
            Assert.NotNull(line.NewLineNumber));
        Assert.Equal([1, 2], block.Lines
            .Where(line => line.Kind == DiffLineKind.Added)
            .Select(line => line.NewLineNumber));
    }

    [Fact]
    public void ParsesDeletedFileEndingAtZero()
    {
        const string diff = """
            diff --git a/src/Old.cs b/src/Old.cs
            deleted file mode 100644
            --- a/src/Old.cs
            +++ /dev/null
            @@ -1,2 +0,0 @@
            -class Old
            -{
            """;

        DiffBlock block = Assert.Single(UnifiedDiffParser.Parse(diff));
        Assert.All(block.Lines.Where(line => line.Kind == DiffLineKind.Deleted), line =>
            Assert.NotNull(line.OldLineNumber));
        Assert.Equal([1, 2], block.Lines
            .Where(line => line.Kind == DiffLineKind.Deleted)
            .Select(line => line.OldLineNumber));
    }

    [Fact]
    public void PairsReplacementLinesForSideBySideView()
    {
        DiffLine[] lines =
        [
            new() { Kind = DiffLineKind.Context, OldLineNumber = 10, NewLineNumber = 10, Text = "before" },
            new() { Kind = DiffLineKind.Deleted, OldLineNumber = 11, Text = "old one" },
            new() { Kind = DiffLineKind.Deleted, OldLineNumber = 12, Text = "old two" },
            new() { Kind = DiffLineKind.Added, NewLineNumber = 11, Text = "new one" },
            new() { Kind = DiffLineKind.Context, OldLineNumber = 13, NewLineNumber = 12, Text = "after" }
        ];

        IReadOnlyList<DiffLinePair> rows = DiffSideBySideBuilder.Build(lines);

        Assert.Equal(4, rows.Count);
        Assert.Equal("before", rows[0].LeftLine!.Text);
        Assert.Same(rows[0].LeftLine, rows[0].RightLine);
        Assert.Equal("old one", rows[1].LeftLine!.Text);
        Assert.Equal("new one", rows[1].RightLine!.Text);
        Assert.Equal("old two", rows[2].LeftLine!.Text);
        Assert.Null(rows[2].RightLine);
        Assert.Equal("after", rows[3].RightLine!.Text);
    }

    [Fact]
    public void KeepsHunksAsFullWidthRows()
    {
        var hunk = new DiffLine
        {
            Kind = DiffLineKind.Hunk,
            Text = "@@ -1,2 +1,3 @@"
        };

        IReadOnlyList<DiffLinePair> rows =
            DiffSideBySideBuilder.Build(
                [hunk, new() { Kind = DiffLineKind.Added, Text = "new" }]);

        Assert.Equal(2, rows.Count);
        Assert.True(rows[0].IsHunk);
        Assert.Same(hunk, rows[0].HunkLine);
        Assert.Equal("new", rows[1].RightLine!.Text);
    }

    [Fact]
    public void DetectsBraceFunctionAndAllowsItToSpanItsBody()
    {
        DiffLine[] lines =
        [
            new() { Kind = DiffLineKind.Context, NewLineNumber = 1, Text = "int Transform(int value)" },
            new() { Kind = DiffLineKind.Context, NewLineNumber = 2, Text = "{" },
            new() { Kind = DiffLineKind.Added, NewLineNumber = 3, Text = "    return value + 1;" },
            new() { Kind = DiffLineKind.Context, NewLineNumber = 4, Text = "}" },
            new() { Kind = DiffLineKind.Context, NewLineNumber = 5, Text = "int Next() { return 2; }" }
        ];

        IReadOnlyList<CodeFoldRegion> regions = CodeFoldBuilder.Build(lines);

        CodeFoldRegion transform = Assert.Single(regions);
        Assert.Equal("Transform", transform.FunctionName);
        Assert.Equal(0, transform.StartIndex);
        Assert.Equal(3, transform.EndIndex);
    }

    [Fact]
    public void DetectsPythonFunctionByIndentation()
    {
        DiffLine[] lines =
        [
            new() { Kind = DiffLineKind.Context, Text = "async def transform(value):" },
            new() { Kind = DiffLineKind.Context, Text = "    if value:" },
            new() { Kind = DiffLineKind.Context, Text = "        return value + 1" },
            new() { Kind = DiffLineKind.Context, Text = "next_value = 0" }
        ];

        CodeFoldRegion region = Assert.Single(CodeFoldBuilder.Build(lines));
        Assert.Equal("transform", region.FunctionName);
        Assert.Equal(0, region.StartIndex);
        Assert.Equal(2, region.EndIndex);
    }
}
