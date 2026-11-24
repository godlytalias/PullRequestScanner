using System.Collections.ObjectModel;
using System.ComponentModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using PullRequestScanner.Core.Exceptions;
using PullRequestScanner.Core.Models;
using PullRequestScanner.Core.Services;
using Windows.Storage;

namespace PullRequestScanner.App.ViewModels;

public sealed partial class MainViewModel : ObservableObject
{
    private readonly ReviewSessionService sessionService;
    private readonly List<ReviewSessionViewModel> allReviews = [];
    private PullRequestReference? authenticationReference;
    private Func<Task>? authenticationRetry;

    public MainViewModel(ReviewSessionService sessionService)
    {
        this.sessionService = sessionService;
        ChangedFilesPaneWidthValue = Math.Clamp(
            GetSavedPaneWidth("ChangedFilesPaneWidth", 240),
            200,
            420);
        ReviewInspectorPaneWidthValue = Math.Clamp(
            GetSavedPaneWidth("ReviewInspectorPaneWidth", 320),
            280,
            500);
        ActiveReview = new ReviewSessionViewModel(sessionService);
        ActiveReview.SetFocusedCommentMode(false);
    }

    [ObservableProperty]
    public partial string PullRequestUrl { get; set; } = string.Empty;

    [ObservableProperty]
    public partial string LocalRepositoryPath { get; set; } = string.Empty;

    [ObservableProperty]
    public partial bool IsOpening { get; set; }

    [ObservableProperty]
    public partial string ErrorMessage { get; set; } = string.Empty;

    [ObservableProperty]
    public partial string StatusMessage { get; set; } = string.Empty;

    [ObservableProperty]
    public partial ReviewSessionViewModel ActiveReview { get; set; }

    [ObservableProperty]
    public partial bool IsCodeViewExpanded { get; set; }

    [ObservableProperty]
    public partial bool IsOpenReviewPanelVisible { get; set; } = true;

    [ObservableProperty]
    public partial bool IsActivityViewVisible { get; set; }

    [ObservableProperty]
    public partial double ChangedFilesPaneWidthValue { get; set; } = 240;

    [ObservableProperty]
    public partial double ReviewInspectorPaneWidthValue { get; set; } = 320;

    public ObservableCollection<ReviewSessionViewModel> OpenReviews { get; } = [];
    public ObservableCollection<ReviewSessionViewModel> PastReviews { get; } = [];
    public int PastReviewCount => PastReviews.Count;
    public bool HasPastReviews => PastReviews.Count > 0;
    public bool HasError => !string.IsNullOrWhiteSpace(ErrorMessage);
    public bool HasReviewAlert => AuthenticationRequired || HasError;
    public bool IsReviewViewVisible => !IsActivityViewVisible;
    public bool IsReviewPanelVisible =>
        IsOpenReviewPanelVisible && IsNormalReviewLayout;
    public ReviewSessionViewModel? SelectedOpenReview =>
        !IsActivityViewVisible && OpenReviews.Contains(ActiveReview)
            ? ActiveReview
            : null;
    public bool IsReviewAlertVisible =>
        HasReviewAlert && IsNormalReviewLayout;
    public bool AuthenticationRequired => authenticationReference is not null;
    public string AuthenticationMessage => authenticationReference is null
        ? string.Empty
        : $"Sign in to {authenticationReference.Provider} for {authenticationReference.Host}, then PullRequest Scanner will retry.";
    public bool CanAuthenticate => AuthenticationRequired && !IsOpening;
    public string CodeViewToggleLabel => IsCodeViewExpanded
        ? "Restore review layout"
        : "Expand code view";
    public string CodeViewToggleGlyph => IsCodeViewExpanded
        ? "\uE73F"
        : "\uE740";
    public bool IsNormalReviewLayout => !IsCodeViewExpanded;
    public GridLength ChangedFilesPaneWidth => IsCodeViewExpanded
        ? new GridLength(0)
        : new GridLength(ChangedFilesPaneWidthValue);
    public GridLength ReviewInspectorPaneWidth => IsCodeViewExpanded
        ? new GridLength(0)
        : new GridLength(ReviewInspectorPaneWidthValue);
    public GridLength PaneSplitterWidth => IsCodeViewExpanded
        ? new GridLength(0)
        : new GridLength(12);
    public NavigationViewPaneDisplayMode NavigationPaneDisplayMode =>
        IsCodeViewExpanded
            ? NavigationViewPaneDisplayMode.LeftCompact
            : NavigationViewPaneDisplayMode.Auto;

    public async Task InitializeAsync()
    {
        try
        {
            IReadOnlyList<ReviewSessionState> states = await sessionService.StateStore.LoadAllAsync();
            foreach (ReviewSessionState state in states)
            {
                TrackReview(new ReviewSessionViewModel(sessionService, state));
            }
            ReclassifyReviews();

            if (allReviews.Count > 0)
            {
                ActiveReview = OpenReviews.FirstOrDefault() ?? allReviews[0];
                IsOpenReviewPanelVisible = false;
                await ActiveReview.EnsureInitialFileSelectedAsync();
            }

            StatusMessage = allReviews.Count == 0
                ? "Open a pull request to begin."
                : $"Restored {allReviews.Count} local review(s).";

            await Task.WhenAll(allReviews.Select(review => review.RefreshSummaryAsync()));
            ReclassifyReviews();
        }
        catch (ReviewOperationException exception)
        {
            ErrorMessage = exception.Message;
        }
        catch (IOException exception)
        {
            ErrorMessage = exception.Message;
        }
    }

    public async Task SelectReviewAsync(ReviewSessionViewModel review)
    {
        if (allReviews.Contains(review))
        {
            ActiveReview = review;
            IsActivityViewVisible = false;
            IsOpenReviewPanelVisible = false;
            StatusMessage = $"Showing {review.NavigationTitle}.";
            ErrorMessage = string.Empty;
            await review.EnsureInitialFileSelectedAsync();
        }
    }

    [RelayCommand(CanExecute = nameof(CanOpenReview))]
    private async Task OpenReviewAsync()
    {
        IsOpening = true;
        ErrorMessage = string.Empty;
        try
        {
            ReviewSessionState state = await sessionService.OpenAsync(
                PullRequestUrl.Trim(),
                string.IsNullOrWhiteSpace(LocalRepositoryPath)
                    ? null
                    : LocalRepositoryPath.Trim());
            ReviewSessionViewModel? existing = OpenReviews.FirstOrDefault(
                review => review.State?.Details.Reference.SessionKey ==
                          state.Details.Reference.SessionKey) ??
                PastReviews.FirstOrDefault(
                    review => review.State?.Details.Reference.SessionKey ==
                              state.Details.Reference.SessionKey);
            if (existing is not null)
            {
                existing.ReplaceState(state);
                await existing.ReopenAsync();
                ActiveReview = existing;
            }

            else
            {
                var review = new ReviewSessionViewModel(sessionService, state);
                TrackReview(review);
                ActiveReview = review;
            }
            ReclassifyReviews();
            IsOpenReviewPanelVisible = false;
            IsActivityViewVisible = false;
            await ActiveReview.EnsureInitialFileSelectedAsync();

            PullRequestUrl = string.Empty;
            LocalRepositoryPath = string.Empty;
            SetAuthenticationRequired(null);
            StatusMessage = $"Opened {ActiveReview.NavigationTitle}.";
        }
        catch (ReviewAuthenticationException exception)
        {
            SetAuthenticationRequired(exception.Reference, OpenReviewAsync);
            ErrorMessage = string.Empty;
            StatusMessage = exception.Message;
        }
        catch (ArgumentException exception)
        {
            ErrorMessage = exception.Message;
        }
        catch (ReviewOperationException exception)
        {
            ErrorMessage = exception.Message;
        }
        catch (HttpRequestException exception)
        {
            ErrorMessage = exception.Message;
        }
        catch (IOException exception)
        {
            ErrorMessage = exception.Message;
        }
        finally
        {
            IsOpening = false;
        }
    }

    [RelayCommand]
    private async Task AuthenticateAndRetryAsync()
    {
        PullRequestReference? reference = authenticationReference;
        if (reference is null)
        {
            return;
        }

        IsOpening = true;
        ErrorMessage = string.Empty;
        StatusMessage = $"Opening {reference.Provider} sign-in…";
        try
        {
            await sessionService.AuthenticateAsync(reference);
            Func<Task>? retry = authenticationRetry;
            SetAuthenticationRequired(null);
            StatusMessage = $"{reference.Provider} sign-in completed. Retrying pull request.";
            IsOpening = false;
            if (retry is not null)
            {
                await retry();
            }
        }
        catch (ReviewOperationException exception)
        {
            ErrorMessage = exception.Message;
        }
        catch (Win32Exception exception)
        {
            ErrorMessage = exception.Message;
        }
        finally
        {
            IsOpening = false;
        }
    }

    [RelayCommand]
    private async Task CloseReview()
    {
        if (ActiveReview.IsEmpty)
        {
            return;
        }

        ReviewSessionViewModel closing = ActiveReview;
        if (OpenReviews.Contains(closing))
        {
            if (await closing.ArchiveAsync())
            {
                ReclassifyReviews();
            }
        }

        ActiveReview = OpenReviews.FirstOrDefault() ??
            PastReviews.FirstOrDefault() ??
            new ReviewSessionViewModel(sessionService);
        IsOpenReviewPanelVisible = ActiveReview.IsEmpty;
        StatusMessage = PastReviews.Contains(closing)
            ? $"Moved {closing.NavigationTitle} to reviewed pull requests."
            : $"Showing {ActiveReview.NavigationTitle}.";
    }

    [RelayCommand]
    private async Task CompleteReview()
    {
        if (ActiveReview.IsEmpty)
        {
            return;
        }

        ReviewSessionViewModel completing = ActiveReview;
        if (!await completing.CompleteAsync())
        {
            ErrorMessage = completing.ErrorMessage;
            return;
        }

        completing.ReviewStateChanged -= Review_ReviewStateChanged;
        completing.AuthenticationRequired -= Review_AuthenticationRequired;
        allReviews.Remove(completing);
        ReclassifyReviews();
        ActiveReview = OpenReviews.FirstOrDefault() ??
            PastReviews.FirstOrDefault() ??
            new ReviewSessionViewModel(sessionService);
        IsOpenReviewPanelVisible = ActiveReview.IsEmpty;
        StatusMessage =
            $"{completing.NavigationTitle} completed. Local review data was cleaned up.";
    }

    [RelayCommand]
    private void ToggleCodeView()
    {
        IsCodeViewExpanded = !IsCodeViewExpanded;
    }

    [RelayCommand]
    private void ShowOpenReviewPanel()
    {
        IsActivityViewVisible = false;
        IsOpenReviewPanelVisible = true;
    }

    [RelayCommand]
    private void HideOpenReviewPanel()
    {
        if (ActiveReview.HasActiveReview)
        {
            IsOpenReviewPanelVisible = false;
        }
    }

    [RelayCommand]
    private void ShowReviewActivity() =>
        IsActivityViewVisible = true;

    [RelayCommand]
    private void HideReviewActivity() =>
        IsActivityViewVisible = false;

    public async Task RefreshOpenReviewsAsync()
    {
        ReviewSessionViewModel[] reviews = OpenReviews.ToArray();
        await Task.WhenAll(reviews.Select(review => review.RefreshFullAsync()));
        ReclassifyReviews();
        if (reviews.Length > 0)
        {
            StatusMessage =
                $"Checked {reviews.Length} active review(s) for updates at {DateTime.Now:t}.";
        }
    }

    public bool CanOpenReview =>
        !IsOpening &&
        !string.IsNullOrWhiteSpace(PullRequestUrl);

    public void ResizeChangedFilesPane(double horizontalChange) =>
        ChangedFilesPaneWidthValue = Math.Clamp(
            ChangedFilesPaneWidthValue + horizontalChange,
            200,
            420);

    public void ResizeReviewInspectorPane(double horizontalChange) =>
        ReviewInspectorPaneWidthValue = Math.Clamp(
            ReviewInspectorPaneWidthValue - horizontalChange,
            280,
            500);

    private void TrackReview(ReviewSessionViewModel review)
    {
        if (allReviews.Contains(review) ||
            allReviews.Any(existing =>
                existing.State?.Details.Reference.SessionKey.Equals(
                    review.State?.Details.Reference.SessionKey,
                    StringComparison.OrdinalIgnoreCase) == true))
        {
            return;
        }

        review.ReviewStateChanged += Review_ReviewStateChanged;
        review.AuthenticationRequired += Review_AuthenticationRequired;
        allReviews.Add(review);
    }

    private void Review_ReviewStateChanged(object? sender, EventArgs args) =>
        ReclassifyReviews();

    private void Review_AuthenticationRequired(
        PullRequestReference reference,
        Func<Task> retry)
    {
        SetAuthenticationRequired(reference, retry);
    }

    private void ReclassifyReviews()
    {
        ReviewSessionViewModel[] open = allReviews
            .Where(review => review.IsOpen)
            .OrderByDescending(review => review.State?.LastOpenedUtc)
            .ToArray();
        ReviewSessionViewModel[] past = allReviews
            .Where(review => review.IsPastReview)
            .OrderByDescending(review =>
                review.State?.Details.UpdatedAtUtc ?? review.State?.LastOpenedUtc)
            .ToArray();
        SynchronizeCollection(OpenReviews, open);
        SynchronizeCollection(PastReviews, past);

        OnPropertyChanged(nameof(PastReviewCount));
        OnPropertyChanged(nameof(HasPastReviews));
        OnPropertyChanged(nameof(SelectedOpenReview));
    }

    private static void SynchronizeCollection<T>(
        ObservableCollection<T> target,
        IReadOnlyList<T> desired)
    {
        for (int index = 0; index < desired.Count; index++)
        {
            if (index < target.Count &&
                ReferenceEquals(target[index], desired[index]))
            {
                continue;
            }

            int existingIndex = target.IndexOf(desired[index]);
            if (existingIndex >= 0)
            {
                target.Move(existingIndex, index);
            }
            else
            {
                target.Insert(index, desired[index]);
            }
        }

        while (target.Count > desired.Count)
        {
            target.RemoveAt(target.Count - 1);
        }
    }

    partial void OnErrorMessageChanged(string value)
    {
        OnPropertyChanged(nameof(HasError));
        OnPropertyChanged(nameof(HasReviewAlert));
        OnPropertyChanged(nameof(IsReviewAlertVisible));
    }

    private void SetAuthenticationRequired(
        PullRequestReference? reference,
        Func<Task>? retry = null)
    {
        authenticationReference = reference;
        authenticationRetry = retry;
        OnPropertyChanged(nameof(AuthenticationRequired));
        OnPropertyChanged(nameof(AuthenticationMessage));
        OnPropertyChanged(nameof(CanAuthenticate));
        OnPropertyChanged(nameof(HasReviewAlert));
        OnPropertyChanged(nameof(IsReviewAlertVisible));
    }

    partial void OnIsOpeningChanged(bool value)
    {
        OnPropertyChanged(nameof(CanAuthenticate));
        OnPropertyChanged(nameof(CanOpenReview));
        OpenReviewCommand.NotifyCanExecuteChanged();
    }

    partial void OnPullRequestUrlChanged(string value)
    {
        OnPropertyChanged(nameof(CanOpenReview));
        OpenReviewCommand.NotifyCanExecuteChanged();
    }

    partial void OnIsCodeViewExpandedChanged(bool value)
    {
        OnPropertyChanged(nameof(CodeViewToggleLabel));
        OnPropertyChanged(nameof(CodeViewToggleGlyph));
        OnPropertyChanged(nameof(IsNormalReviewLayout));
        OnPropertyChanged(nameof(IsReviewPanelVisible));
        OnPropertyChanged(nameof(IsReviewAlertVisible));
        OnPropertyChanged(nameof(ChangedFilesPaneWidth));
        OnPropertyChanged(nameof(ReviewInspectorPaneWidth));
        OnPropertyChanged(nameof(PaneSplitterWidth));
        OnPropertyChanged(nameof(NavigationPaneDisplayMode));
        ActiveReview.SetFocusedCommentMode(value);
    }

    partial void OnChangedFilesPaneWidthValueChanged(double value)
    {
        OnPropertyChanged(nameof(ChangedFilesPaneWidth));
        SavePaneWidth("ChangedFilesPaneWidth", value);
    }

    partial void OnReviewInspectorPaneWidthValueChanged(double value)
    {
        OnPropertyChanged(nameof(ReviewInspectorPaneWidth));
        SavePaneWidth("ReviewInspectorPaneWidth", value);
    }

    partial void OnActiveReviewChanged(ReviewSessionViewModel value)
    {
        value.SetFocusedCommentMode(IsCodeViewExpanded);
        OnPropertyChanged(nameof(SelectedOpenReview));
        if (!value.IsEmpty)
        {
            IsOpenReviewPanelVisible = false;
        }
    }

    partial void OnIsOpenReviewPanelVisibleChanged(bool value) =>
        OnPropertyChanged(nameof(IsReviewPanelVisible));

    partial void OnIsActivityViewVisibleChanged(bool value)
    {
        OnPropertyChanged(nameof(IsReviewViewVisible));
        OnPropertyChanged(nameof(SelectedOpenReview));
    }

    private static double GetSavedPaneWidth(
        string settingName,
        double fallback)
    {
        try
        {
            return ApplicationData.Current.LocalSettings.Values[settingName] is double saved
                ? saved
                : fallback;
        }
        catch (InvalidOperationException)
        {
            return fallback;
        }
    }

    private static void SavePaneWidth(string settingName, double value)
    {
        try
        {
            ApplicationData.Current.LocalSettings.Values[settingName] = value;
        }
        catch (InvalidOperationException)
        {
        }
    }
}
