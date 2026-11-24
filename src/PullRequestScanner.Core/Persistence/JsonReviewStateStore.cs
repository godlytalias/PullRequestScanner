using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using PullRequestScanner.Core.Exceptions;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Persistence;

public sealed class JsonReviewStateStore(string rootPath)
{
    private readonly SemaphoreSlim writeGate = new(1, 1);

    private static readonly JsonSerializerOptions Options = new()
    {
        WriteIndented = true,
        PropertyNameCaseInsensitive = true,
        Converters = { new JsonStringEnumConverter() }
    };

    public string RootPath { get; } = Path.GetFullPath(rootPath);

    public async Task<ReviewSessionState?> LoadAsync(
        string sessionKey,
        CancellationToken cancellationToken = default)
    {
        string path = GetPath(sessionKey);
        if (!File.Exists(path))
        {
            return null;
        }

        try
        {
            await using FileStream stream = File.OpenRead(path);
            ReviewSessionState? state = await JsonSerializer.DeserializeAsync<ReviewSessionState>(
                stream,
                Options,
                cancellationToken);
            return state is null ? null : Migrate(state);
        }
        catch (JsonException exception)
        {
            throw new ReviewOperationException(
                $"The saved review state is invalid: {path}. Move or repair this file before reopening the review.",
                exception);
        }
    }

    public async Task<IReadOnlyList<ReviewSessionState>> LoadAllAsync(
        CancellationToken cancellationToken = default)
    {
        if (!Directory.Exists(RootPath))
        {
            return [];
        }

        var states = new List<ReviewSessionState>();
        foreach (string path in Directory.EnumerateFiles(RootPath, "*.json", SearchOption.TopDirectoryOnly))
        {
            try
            {
                await using FileStream stream = File.OpenRead(path);
                ReviewSessionState? state = await JsonSerializer.DeserializeAsync<ReviewSessionState>(
                    stream,
                    Options,
                    cancellationToken);
                if (state is not null)
                {
                    states.Add(Migrate(state));
                }
            }
            catch (JsonException exception)
            {
                throw new ReviewOperationException(
                    $"The saved review state is invalid: {path}. Move or repair this file before starting the app.",
                    exception);
            }
        }

        return states
            .OrderByDescending(state => state.LastOpenedUtc)
            .ToArray();
    }

    public async Task SaveAsync(
        ReviewSessionState state,
        CancellationToken cancellationToken = default)
    {
        await writeGate.WaitAsync(cancellationToken);
        string? temporary = null;
        try
        {
            Directory.CreateDirectory(RootPath);
            string destination = GetPath(state.Details.Reference.SessionKey);
            temporary = destination + "." + Guid.NewGuid().ToString("N") + ".tmp";

            await using (FileStream stream = File.Create(temporary))
            {
                await JsonSerializer.SerializeAsync(
                    stream,
                    state,
                    Options,
                    cancellationToken);
                await stream.FlushAsync(cancellationToken);
            }

            File.Move(temporary, destination, overwrite: true);
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

    public async Task DeleteAsync(
        ReviewSessionState state,
        CancellationToken cancellationToken = default)
    {
        await writeGate.WaitAsync(cancellationToken);
        try
        {
            string path = GetPath(state.Details.Reference.SessionKey);
            if (File.Exists(path))
            {
                File.Delete(path);
            }
        }
        finally
        {
            writeGate.Release();
        }
    }

    public string GetPath(string sessionKey)
    {
        string hash = Convert.ToHexString(
            SHA256.HashData(Encoding.UTF8.GetBytes(sessionKey)))[..24].ToLowerInvariant();
        return Path.Combine(RootPath, $"{hash}.json");
    }

    private static ReviewSessionState Migrate(ReviewSessionState state)
    {
        state.SchemaVersion = Math.Max(state.SchemaVersion, 5);
        state.ViewedFiles ??= [];
        state.ChangedFiles ??= [];
        state.Comments ??= [];
        state.RemoteComments ??= [];
        state.Iterations ??= [];
        return state;
    }
}
