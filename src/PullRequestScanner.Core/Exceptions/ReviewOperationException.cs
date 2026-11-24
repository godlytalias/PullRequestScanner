using PullRequestScanner.Core.Models;

namespace PullRequestScanner.Core.Exceptions;

public class ReviewOperationException : Exception
{
    public ReviewOperationException(string message)
        : base(message)
    {
    }
    public ReviewOperationException(string message, Exception innerException)
        : base(message, innerException)
    {
    }
}

public sealed class ReviewAuthenticationException : ReviewOperationException
{
    public ReviewAuthenticationException(
        PullRequestReference reference,
        string message)
        : base(message)
    {
        Reference = reference;
    }

    public PullRequestReference Reference { get; }
}
