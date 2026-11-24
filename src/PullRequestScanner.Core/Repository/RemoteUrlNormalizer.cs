namespace PullRequestScanner.Core.Repository;

public static class RemoteUrlNormalizer
{
    public static string Normalize(string? value)
    {
        if (string.IsNullOrWhiteSpace(value))
        {
            return string.Empty;
        }

        string input = value.Trim();
        if (input.StartsWith("git@", StringComparison.OrdinalIgnoreCase))
        {
            int colonIndex = input.IndexOf(':');
            if (colonIndex > 0)
            {
                input = $"https://{input[4..colonIndex]}/{input[(colonIndex + 1)..]}";
            }
        }

        if (!Uri.TryCreate(input, UriKind.Absolute, out Uri? uri))
        {
            return NormalizePath(input);
        }

        string path = Uri.UnescapeDataString(uri.AbsolutePath)
            .Replace('\\', '/')
            .TrimEnd('/');
        if (path.EndsWith(".git", StringComparison.OrdinalIgnoreCase))
        {
            path = path[..^4];
        }

        return $"{uri.Host.ToLowerInvariant()}/{path.TrimStart('/')}".ToLowerInvariant();
    }

    private static string NormalizePath(string value)
    {
        string path = value.Replace('\\', '/').TrimEnd('/');
        if (path.EndsWith(".git", StringComparison.OrdinalIgnoreCase))
        {
            path = path[..^4];
        }

        return path.ToLowerInvariant();
    }
}
