using CommunityToolkit.Mvvm.Input;

namespace PullRequestScanner.App.ViewModels;

public sealed class DiffChangeMarkerViewModel
{
    public DiffChangeMarkerViewModel(
        double position,
        string label,
        DiffLineViewModel target,
        Action<DiffLineViewModel> navigate,
        bool isAddition,
        bool isComment = false)
    {
        Position = Math.Clamp(position, 0, 1);
        Label = label;
        Target = target;
        IsAddition = isAddition;
        IsComment = isComment;
        NavigateCommand = new RelayCommand(() => navigate(target));
    }

    public double Position { get; }
    public string Label { get; }
    public DiffLineViewModel Target { get; }
    public bool IsAddition { get; }
    public bool IsDeletion => !IsAddition && !IsComment;
    public bool IsComment { get; }
    public IRelayCommand NavigateCommand { get; }
}
