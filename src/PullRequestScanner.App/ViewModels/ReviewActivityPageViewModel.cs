using System.Collections.ObjectModel;
using System.Globalization;
using System.Text.Json;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using PullRequestScanner.Core.Services;
using PullRequestScanner.Core.Models;

namespace PullRequestScanner.App.ViewModels;

public sealed record ReviewActivityDayViewModel
{
    public required DateOnly Date { get; init; }
    public int PullRequestCount { get; init; }
    public int CommentCount { get; init; }
    public int ReviewCount { get; init; }
    public double PullRequestBarHeight { get; init; }
    public double CommentBarHeight { get; init; }
    public double ReviewBarHeight { get; init; }

    public string DayLabel => Date.Day.ToString(
        CultureInfo.CurrentCulture);

    public string Tooltip => $"{Date:d}: {PullRequestCount} pull request(s), " +
        $"{CommentCount} comment(s), {ReviewCount} review(s) completed";
}

public sealed record RecentReviewActivityViewModel
{
    public required ReviewActivityRecord Record { get; init; }
    public string Title => Record.PullRequestTitle;
    public string Location =>
        $"{Record.Repository} #{Record.PullRequestNumber}";
    public string Status => Record.Kind == ReviewActivityKind.Completed
        ? "Completed"
        : "Marked reviewed";
    public string ActivityDate => Record.AtUtc
        .ToLocalTime()
        .ToString("d", CultureInfo.CurrentCulture);
}

public sealed partial class ReviewActivityPageViewModel(
    ReviewActivityService activityService) : ObservableObject
{
    private DateOnly visibleMonth = new(
        DateTime.Today.Year,
        DateTime.Today.Month,
        1);

    public ObservableCollection<ReviewActivityDayViewModel> Days { get; } = [];
    public ObservableCollection<RecentReviewActivityViewModel> RecentReviews { get; } = [];
    public bool HasRecentReviews => RecentReviews.Count > 0;
    public bool HasNoRecentReviews => !HasRecentReviews;

    [ObservableProperty]
    public partial string MonthLabel { get; set; } = string.Empty;

    [ObservableProperty]
    public partial int PullRequestTotal { get; set; }

    [ObservableProperty]
    public partial int CommentTotal { get; set; }

    [ObservableProperty]
    public partial int ReviewTotal { get; set; }

    [ObservableProperty]
    public partial int ChartMaximum { get; set; } = 5;

    [ObservableProperty]
    public partial bool IsBusy { get; set; }

    [ObservableProperty]
    public partial string ErrorMessage { get; set; } = string.Empty;

    public bool HasError => !string.IsNullOrWhiteSpace(ErrorMessage);
    public int ChartMidpoint => ChartMaximum / 2;
    public string ChartMidpointLabel =>
        (ChartMaximum / 2d).ToString("0.##", CultureInfo.CurrentCulture);
    public bool CanMoveToNextMonth =>
        visibleMonth < new DateOnly(
            DateTime.Today.Year,
            DateTime.Today.Month,
            1);

    public Task InitializeAsync() => LoadMonthAsync();

    [RelayCommand]
    private Task PreviousMonthAsync() =>
        LoadMonthAsync(visibleMonth.AddMonths(-1));

    [RelayCommand(CanExecute = nameof(CanMoveToNextMonth))]
    private Task NextMonthAsync() =>
        LoadMonthAsync(visibleMonth.AddMonths(1));

    private async Task LoadMonthAsync(DateOnly? month = null)
    {
        if (month is DateOnly target)
        {
            visibleMonth = target;
        }

        IsBusy = true;
        ErrorMessage = string.Empty;
        try
        {
            IReadOnlyList<ReviewActivityDay> days =
                await activityService.GetMonthAsync(
                    visibleMonth.Year,
                    visibleMonth.Month);
            int scaleMaximum = RoundChartMaximum(
                days.SelectMany(day => new[]
                    {
                        day.PullRequestCount,
                        day.CommentCount,
                        day.ReviewCount
                    })
                    .Max());

            Days.Clear();
            foreach (ReviewActivityDay day in days)
            {
                Days.Add(new ReviewActivityDayViewModel
                {
                    Date = day.Date,
                    PullRequestCount = day.PullRequestCount,
                    CommentCount = day.CommentCount,
                    ReviewCount = day.ReviewCount,
                    PullRequestBarHeight =
                        ScaleHeight(day.PullRequestCount, scaleMaximum),
                    CommentBarHeight =
                        ScaleHeight(day.CommentCount, scaleMaximum),
                    ReviewBarHeight =
                        ScaleHeight(day.ReviewCount, scaleMaximum)
                });
            }

            RecentReviews.Clear();
            foreach (ReviewActivityRecord record in
                     await activityService.GetRecentReviewsAsync())
            {
                RecentReviews.Add(new RecentReviewActivityViewModel
                {
                    Record = record
                });
            }
            OnPropertyChanged(nameof(HasRecentReviews));
            OnPropertyChanged(nameof(HasNoRecentReviews));

            MonthLabel = visibleMonth
                .ToDateTime(TimeOnly.MinValue)
                .ToString("MMMM yyyy", CultureInfo.CurrentCulture);
            PullRequestTotal = days.Sum(day => day.PullRequestCount);
            CommentTotal = days.Sum(day => day.CommentCount);
            ReviewTotal = days.Sum(day => day.ReviewCount);
            ChartMaximum = scaleMaximum;
            OnPropertyChanged(nameof(ChartMidpoint));
            OnPropertyChanged(nameof(ChartMidpointLabel));
            NextMonthCommand.NotifyCanExecuteChanged();
            OnPropertyChanged(nameof(CanMoveToNextMonth));
        }
        catch (IOException exception)
        {
            ErrorMessage = exception.Message;
        }
        catch (JsonException exception)
        {
            ErrorMessage = exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    private static double ScaleHeight(int value, int maximum) =>
        value == 0
            ? 0
            : Math.Max(4, value * 152d / maximum);

    private static int RoundChartMaximum(int maximum) =>
        Math.Max(5, (int)Math.Ceiling(maximum / 5d) * 5);

    partial void OnErrorMessageChanged(string value) =>
        OnPropertyChanged(nameof(HasError));
}
