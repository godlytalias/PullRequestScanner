using System.Net;
using System.Net.Http.Headers;
using System.Text;
using System.Text.Json;
using PullRequestScanner.Core.Authentication;
using PullRequestScanner.Core.Exceptions;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Providers;

public sealed class ReviewProviderHttp(
    HttpClient httpClient,
    ICredentialProvider credentialProvider)
{
    public static JsonSerializerOptions JsonOptions { get; } = new()
    {
        PropertyNameCaseInsensitive = true
    };

    public async Task<JsonDocument> GetJsonAsync(
        PullRequestReference reference,
        string requestUri,
        CancellationToken cancellationToken)
    {
        using HttpRequestMessage request = await CreateRequestAsync(
            reference,
            HttpMethod.Get,
            requestUri,
            content: null,
            cancellationToken);
        using HttpResponseMessage response = await httpClient.SendAsync(request, cancellationToken);
        string body = await ReadAndValidateAsync(reference, response, cancellationToken);
        return JsonDocument.Parse(body);
    }

    public async Task<JsonDocument> SendJsonAsync(
        PullRequestReference reference,
        HttpMethod method,
        string requestUri,
        object payload,
        CancellationToken cancellationToken)
    {
        string json = JsonSerializer.Serialize(payload, JsonOptions);
        using var content = new StringContent(json, Encoding.UTF8, "application/json");
        using HttpRequestMessage request = await CreateRequestAsync(
            reference,
            method,
            requestUri,
            content,
            cancellationToken);
        using HttpResponseMessage response = await httpClient.SendAsync(request, cancellationToken);
        string body = await ReadAndValidateAsync(reference, response, cancellationToken);
        return JsonDocument.Parse(body);
    }

    public async Task DeleteAsync(
        PullRequestReference reference,
        string requestUri,
        CancellationToken cancellationToken)
    {
        using HttpRequestMessage request = await CreateRequestAsync(
            reference,
            HttpMethod.Delete,
            requestUri,
            content: null,
            cancellationToken);
        using HttpResponseMessage response = await httpClient.SendAsync(request, cancellationToken);
        await ReadAndValidateAsync(reference, response, cancellationToken);
    }

    private async Task<HttpRequestMessage> CreateRequestAsync(
        PullRequestReference reference,
        HttpMethod method,
        string requestUri,
        HttpContent? content,
        CancellationToken cancellationToken)
    {
        var request = new HttpRequestMessage(method, requestUri)
        {
            Content = content
        };

        request.Headers.Accept.Add(new MediaTypeWithQualityHeaderValue("application/json"));
        request.Headers.UserAgent.ParseAdd("PullRequest-Scanner/1.0");

        ReviewCredential? credential = await credentialProvider.GetCredentialAsync(
            reference,
            cancellationToken);
        if (credential is not null)
        {
            request.Headers.Authorization = new AuthenticationHeaderValue(
                credential.Scheme,
                credential.Parameter);
        }

        if (reference.Provider == ReviewProvider.GitHub)
        {
            request.Headers.TryAddWithoutValidation("X-GitHub-Api-Version", "2022-11-28");
        }

        return request;
    }

    private static async Task<string> ReadAndValidateAsync(
        PullRequestReference reference,
        HttpResponseMessage response,
        CancellationToken cancellationToken)
    {
        string body = await response.Content.ReadAsStringAsync(cancellationToken);
        if (response.IsSuccessStatusCode)
        {
            return body;
        }

        if (response.StatusCode is HttpStatusCode.Unauthorized or HttpStatusCode.Forbidden)
        {
            string authenticationHint = reference.Provider == ReviewProvider.GitHub
                ? $"Run 'gh auth login --hostname {reference.Host}' or set GH_TOKEN."
                : "Run 'az login' or set AZURE_DEVOPS_EXT_PAT.";
            throw new ReviewAuthenticationException(
                reference,
                $"{reference.Provider} rejected the request ({(int)response.StatusCode}). {authenticationHint}");
        }

        string details = body.Length > 800 ? body[..800] : body;
        throw new ReviewOperationException(
            $"{reference.Provider} request failed with HTTP {(int)response.StatusCode} ({response.ReasonPhrase}). {details}");
    }
}
