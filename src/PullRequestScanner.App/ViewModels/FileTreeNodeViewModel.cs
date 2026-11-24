using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.App.ViewModels;

public sealed partial class FileTreeNodeViewModel : ObservableObject
{
    public FileTreeNodeViewModel(string name, string path, bool isFolder, ChangedFile? changedFile = null)
    {
        Name = name;
        Path = path;
        IsFolder = isFolder;
        ChangedFile = changedFile;
    }

    public string Name { get; private set; }
    public string Path { get; private set; }
    public bool IsFolder { get; }
    public bool IsFile => !IsFolder;
    public ChangedFile? ChangedFile { get; }
    public ObservableCollection<FileTreeNodeViewModel> Children { get; } = [];
    public string Glyph => IsFolder ? "\uE8B7" : "\uE8A5";
    public FileTreeNodeViewModel? Parent { get; private set; }

    [ObservableProperty]
    public partial bool IsExpanded { get; set; }
    [ObservableProperty]
    public partial bool IsViewed { get; set; }
    public string Status => ChangedFile?.Status switch
    {
        ChangedFileStatus.Added => "A",
        ChangedFileStatus.Deleted => "D",
        ChangedFileStatus.Renamed => "R",
        ChangedFileStatus.Copied => "C",
        ChangedFileStatus.TypeChanged => "T",
        ChangedFileStatus.Unmerged => "U",
        _ => "M"
    };
    public bool IsAddedStatus => ChangedFile?.Status == ChangedFileStatus.Added;
    public bool IsModifiedStatus => ChangedFile?.Status == ChangedFileStatus.Modified;
    public bool IsDeletedStatus => ChangedFile?.Status == ChangedFileStatus.Deleted;
    public bool IsOtherStatus => IsFile &&
        !IsAddedStatus &&
        !IsModifiedStatus &&
        !IsDeletedStatus;
    public bool IsUnviewed => !IsViewed;
    public string AccessibilityName => IsFile
        ? $"{ChangedFile?.Status} file {Path}" +
          (IsViewed ? ", viewed" : string.Empty)
        : $"Folder {Path}";

    public void SortChildrenRecursively()
    {
        FileTreeNodeViewModel[] sorted = Children
            .OrderByDescending(child => child.IsFolder)
            .ThenBy(child => child.Name, StringComparer.OrdinalIgnoreCase)
            .ToArray();
        Children.Clear();
        foreach (FileTreeNodeViewModel child in sorted)
        {
            child.SortChildrenRecursively();
            Children.Add(child);
        }
    }

    public void CompactSingleChildFolders()
    {
        if (!IsFolder)
        {
            return;
        }

        while (Children.Count == 1 &&
               Children[0].IsFolder)
        {
            FileTreeNodeViewModel child = Children[0];
            Name = $"{Name}/{child.Name}";
            Path = child.Path;
            FileTreeNodeViewModel[] grandchildren = child.Children.ToArray();
            Children.Clear();
            foreach (FileTreeNodeViewModel grandchild in grandchildren)
            {
                grandchild.SetParent(this);
                Children.Add(grandchild);
            }
        }

        foreach (FileTreeNodeViewModel child in Children)
        {
            child.CompactSingleChildFolders();
        }
    }

    public void ExpandAncestors()
    {
        FileTreeNodeViewModel? parent = Parent;
        while (parent is not null)
        {
            parent.IsExpanded = true;
            parent = parent.Parent;
        }

    }

    public void SetViewed(bool viewed) =>
        IsViewed = viewed;

    partial void OnIsViewedChanged(bool value)
    {
        OnPropertyChanged(nameof(AccessibilityName));
        OnPropertyChanged(nameof(IsUnviewed));
    }

    public override string ToString() => IsFile
        ? $"{Status}: {Path}"
        : Path;

    internal void SetParent(FileTreeNodeViewModel? parent) =>
        Parent = parent;
}

public static class FileTreeBuilder
{
    public static ObservableCollection<FileTreeNodeViewModel> Build(
        IEnumerable<ChangedFile> changedFiles,
        IEnumerable<string>? viewedPaths = null)
    {
        HashSet<string> viewed = viewedPaths?
            .ToHashSet(StringComparer.OrdinalIgnoreCase) ?? [];
        var roots = new ObservableCollection<FileTreeNodeViewModel>();
        foreach (ChangedFile file in changedFiles.OrderBy(file => file.Path, StringComparer.OrdinalIgnoreCase))
        {
            string[] segments = file.Path.Replace('\\', '/').Split(
                '/',
                StringSplitOptions.RemoveEmptyEntries);
            if (segments.Length == 0)
            {
                continue;
            }

            ObservableCollection<FileTreeNodeViewModel> siblings = roots;
            string currentPath = string.Empty;
            for (int index = 0; index < segments.Length; index++)
            {
                bool isLast = index == segments.Length - 1;
                currentPath = string.IsNullOrEmpty(currentPath)
                    ? segments[index]
                    : $"{currentPath}/{segments[index]}";
                FileTreeNodeViewModel? node = siblings.FirstOrDefault(
                    candidate => candidate.Name.Equals(
                        segments[index],
                        StringComparison.OrdinalIgnoreCase));
                if (node is null)
                {
                    node = new FileTreeNodeViewModel(
                        segments[index],
                        currentPath,
                        !isLast,
                        isLast ? file : null);
                    if (isLast)
                    {
                        node.SetViewed(viewed.Contains(node.Path));
                    }
                    siblings.Add(node);
                    FileTreeNodeViewModel? parent = ReferenceEquals(roots, siblings)
                        ? null
                        : FindParent(roots, currentPath, segments[index]);
                    node.SetParent(parent);
                }

                siblings = node.Children;
            }
        }

        foreach (FileTreeNodeViewModel root in roots)
        {
            root.CompactSingleChildFolders();
        }

        FileTreeNodeViewModel[] sortedRoots = roots
            .OrderByDescending(node => node.IsFolder)
            .ThenBy(node => node.Name, StringComparer.OrdinalIgnoreCase)
            .ToArray();
        roots.Clear();
        foreach (FileTreeNodeViewModel root in sortedRoots)
        {
            root.SortChildrenRecursively();
            roots.Add(root);
        }

        return roots;
    }

    private static FileTreeNodeViewModel? FindParent(
        IEnumerable<FileTreeNodeViewModel> roots,
        string childPath,
        string childName)
    {
        string separator = "/" + childName;
        if (!childPath.EndsWith(separator, StringComparison.OrdinalIgnoreCase))
        {
            return null;
        }

        string parentPath = childPath[..^separator.Length];
        return parentPath.Length == 0
            ? null
            : FindNode(roots, parentPath);
    }

    private static FileTreeNodeViewModel? FindNode(
        IEnumerable<FileTreeNodeViewModel> roots,
        string path)
    {
        foreach (FileTreeNodeViewModel node in roots)
        {
            if (node.Path.Equals(path, StringComparison.OrdinalIgnoreCase))
            {
                return node;
            }

            FileTreeNodeViewModel? nested = FindNode(node.Children, path);
            if (nested is not null)
            {
                return nested;
            }
        }

        return null;
    }
}
