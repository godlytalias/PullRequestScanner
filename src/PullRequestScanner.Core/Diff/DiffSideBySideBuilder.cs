using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Diff;

public static class DiffSideBySideBuilder
{
    public static IReadOnlyList<DiffLinePair> Build(
        IReadOnlyList<DiffLine> lines)
    {
        var result = new List<DiffLinePair>();
        var deleted = new List<DiffLine>();
        var added = new List<DiffLine>();

        void FlushChanges()
        {
            int count = Math.Max(deleted.Count, added.Count);
            for (int index = 0; index < count; index++)
            {
                result.Add(new DiffLinePair(
                    index < deleted.Count ? deleted[index] : null,
                    index < added.Count ? added[index] : null,
                    null));
            }

            deleted.Clear();
            added.Clear();
        }

        foreach (DiffLine line in lines)
        {
            switch (line.Kind)
            {
                case DiffLineKind.Deleted:
                    deleted.Add(line);
                    break;
                case DiffLineKind.Added:
                    added.Add(line);
                    break;
                case DiffLineKind.Hunk:
                    FlushChanges();
                    result.Add(new DiffLinePair(null, null, line));
                    break;
                case DiffLineKind.Context:
                    FlushChanges();
                    result.Add(new DiffLinePair(line, line, null));
                    break;
            }
        }

        FlushChanges();
        return result;
    }
}
