using System.Text.Json;
using System.Text.Json.Serialization;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Persistence;

public sealed class JsonReviewActivityStore(string path)
{
    private readonly SemaphoreSlim writeGate = new(1, 1);
    private static readonly JsonSerializerOptions Options = new()
    {
        WriteIndented = true,
        Converters = { new JsonStringEnumConverter() }
    };

    public string Path { get; } = System.IO.Path.GetFullPath(path);

    public async Task<IReadOnlyList<ReviewActivityRecord>> LoadAsync(
        CancellationToken cancellationToken = default)
    {
        if (!File.Exists(Path))
        {
            return [];
        }

        await using FileStream stream = File.OpenRead(Path);
        ReviewActivityStoreState? state = await JsonSerializer.DeserializeAsync<ReviewActivityStoreState>(
            stream,
            Options,
            cancellationToken);
        return state?.Records ?? [];
    }

    public async Task RecordAsync(
        ReviewActivityRecord record,
        CancellationToken cancellationToken = default)
    {
        await writeGate.WaitAsync(cancellationToken);
        string? temporary = null;
        try
        {
            Directory.CreateDirectory(System.IO.Path.GetDirectoryName(Path)!);
            ReviewActivityStoreState state = await LoadStateUnsafeAsync(cancellationToken);
            if (state.Records.Any(existing =>
                    existing.Id.Equals(record.Id, StringComparison.Ordinal)))
            {
                return;
            }

            state.Records.Add(record);
            temporary = Path + "." + Guid.NewGuid().ToString("N") + ".tmp";
            await using (FileStream stream = File.Create(temporary))
            {
                await JsonSerializer.SerializeAsync(
                    stream,
                    state,
                    Options,
                    cancellationToken);
                await stream.FlushAsync(cancellationToken);
            }

            File.Move(temporary, Path, overwrite: true);
        }
        finally
        {
            if (temporary is not null && File.Exists(temporary))
            {
                File.Delete(temporary);
            }

            writeGate.Release();
        }
    }

    private async Task<ReviewActivityStoreState> LoadStateUnsafeAsync(
        CancellationToken cancellationToken)
    {
        if (!File.Exists(Path))
        {
            return new ReviewActivityStoreState();
        }

        await using FileStream stream = File.OpenRead(Path);
        return await JsonSerializer.DeserializeAsync<ReviewActivityStoreState>(
            stream,
            Options,
            cancellationToken) ??
            new ReviewActivityStoreState();
    }
}
