mod controller;
mod native;
mod raw;

use crate::resources::Resources;
use naui::{
    Canvas, FilePicker, FilePickerMode, Length, Result, Sizing, Table, TableColumn, Tasks, Tree,
    Ui, Window,
};
use prs_winui_projection::Microsoft::UI::Xaml::Controls::Primitives::ToggleButton as XamlToggleButton;
use prs_winui_projection::Microsoft::UI::Xaml::Controls::{
    AppBarButton, Button as XamlButton, ColumnDefinition, ComboBox as XamlComboBox, ContentControl,
    ContentDialog, Expander, FontIcon, Grid as XamlGrid, HyperlinkButton, InfoBar as XamlInfoBar,
    ListView, MenuFlyoutItem, NavigationView, ProgressBar as XamlProgressBar,
    ProgressRing as XamlProgressRing, StackPanel as XamlStackPanel, TextBlock as XamlTextBlock,
    TextBox as XamlTextBox,
};
use prs_winui_projection::Microsoft::UI::Xaml::{
    FrameworkElement, UIElement, Window as XamlWindow,
};
use windows_core::{HSTRING, Interface};

use native::{DiffList, Pane, PaneSplitter, Sidebar, SidebarReviewItem, Tabs, Toolbar};
use raw::{
    Button, ComboBox, Hyperlink, InfoBar, Label, MenuItem, ProgressBar, RawSheet, RawWidget,
    TextInput,
};

const WINDOW_WIDTH: f64 = 1600.0;
const WINDOW_HEIGHT: f64 = 1000.0;
const SIDEBAR_WIDTH: f64 = 240.0;
const SHELL_XAML: &str = include_str!("../resources/Shell.xaml");

pub use controller::build;

struct AppShell {
    window: Window,
    review_shell: Pane,
    open_review: OpenReviewSheet,
    toolbar: Toolbar,
    sidebar: Sidebar,
    empty: Pane,
    activity: ActivityPane,
    workspace: ReviewWorkspace,
    status: StatusPane,
    description: DescriptionSheet,
    completion: CompletionSheet,
    deletion: DeletionSheet,
    focused_comment: FocusedCommentSheet,
}

impl AppShell {
    fn new(ui: &Ui) -> Result<Self> {
        let tasks = ui.tasks();
        let resources = Resources::en_us();
        let window = ui.window(
            resource(resources, "App.Title")?,
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
        )?;
        window.set_min_size(960.0, 640.0);
        window.set_state(naui::WindowState::Maximized);
        let native_window = window.backend().native_window();

        let shell = raw::load::<XamlGrid>(SHELL_XAML)?;
        let shell_root = shell
            .cast::<FrameworkElement>()
            .map_err(|error| naui::Error::new("get the raw WinUI shell root", error.to_string()))?;
        let shell_element = shell
            .cast::<UIElement>()
            .map_err(|error| naui::Error::new("host the raw WinUI shell", error.to_string()))?;
        let host = ui.native_host()?;
        host.set_sizing(Sizing::fill());
        if !host.backend().set_hosted_element(Some(&shell_element)) {
            return Err(naui::Error::new(
                "host the raw WinUI shell",
                "WinUI refused the shell element",
            ));
        }
        window.set_child(&host);

        let open_pr_title_button = raw::mount_title_bar_action(
            tasks.clone(),
            &native_window,
            resource(resources, "App.OpenPr")?,
            resource(resources, "App.OpenPrName")?,
            resource(resources, "App.OpenPrTooltip")?,
        )?;

        let review_shell = pane(&shell_root, "ReviewWorkspace")?;
        let open_review =
            OpenReviewSheet::new(ui, tasks.clone(), &shell_root, native_window.clone())?;
        let toolbar = toolbar(tasks.clone(), &shell_root, open_pr_title_button)?;
        let sidebar = sidebar(ui, &shell_root)?;
        let status = StatusPane::new(tasks.clone(), &shell_root)?;
        let activity = ActivityPane::new(ui, tasks.clone(), &shell_root)?;
        let workspace = ReviewWorkspace::new(ui, tasks.clone(), &shell_root)?;
        let empty = pane(&shell_root, "EmptyReviewPanel")?;
        let description = DescriptionSheet::new(tasks.clone(), native_window.clone(), &shell_root)?;
        let completion = CompletionSheet::new(tasks.clone(), native_window.clone(), &shell_root)?;
        let deletion = DeletionSheet::new(tasks.clone(), native_window.clone(), &shell_root)?;
        let focused_comment = FocusedCommentSheet::new(tasks.clone(), native_window, &shell_root)?;

        initialize_shell_strings(tasks, &shell_root, resources)?;
        workspace.root.set_visible(false);
        activity.root.set_visible(false);

        Ok(Self {
            window,
            review_shell,
            open_review,
            toolbar,
            sidebar,
            empty,
            activity,
            workspace,
            status,
            description,
            completion,
            deletion,
            focused_comment,
        })
    }

    fn show(&self) {
        self.window.show();
    }
}

fn resource(resources: Resources, key: &'static str) -> Result<&'static str> {
    resources
        .text(key)
        .map_err(|error| naui::Error::new("load a localized string", error.to_string()))
}

fn text_input(tasks: Tasks, root: &FrameworkElement, name: &str) -> Result<TextInput> {
    Ok(TextInput::from_native(
        tasks,
        raw::element::<XamlTextBox>(root, name)?,
    ))
}

fn label(root: &FrameworkElement, name: &str) -> Result<Label> {
    Ok(Label::from_native(raw::element::<XamlTextBlock>(
        root, name,
    )?))
}

fn button(tasks: Tasks, root: &FrameworkElement, name: &str) -> Result<Button> {
    Ok(Button::from_button(
        tasks,
        raw::element::<XamlButton>(root, name)?,
    ))
}

fn toggle_button(tasks: Tasks, root: &FrameworkElement, name: &str) -> Result<Button> {
    Ok(Button::from_toggle(
        tasks,
        raw::element::<XamlToggleButton>(root, name)?,
    ))
}

fn app_bar_button(tasks: Tasks, root: &FrameworkElement, name: &str) -> Result<Button> {
    Ok(Button::from_app_bar(
        tasks,
        raw::element::<AppBarButton>(root, name)?,
    ))
}

fn pane(root: &FrameworkElement, name: &str) -> Result<Pane> {
    Ok(Pane::from_element(raw::ui_element(&raw::element::<
        FrameworkElement,
    >(root, name)?)?))
}

fn info_bar(tasks: Tasks, root: &FrameworkElement, name: &str) -> Result<InfoBar> {
    Ok(InfoBar::from_native(
        tasks,
        raw::element::<XamlInfoBar>(root, name)?,
    ))
}

fn menu_item(tasks: Tasks, root: &FrameworkElement, name: &str) -> Result<MenuItem> {
    Ok(MenuItem::from_native(
        tasks,
        raw::element::<MenuFlyoutItem>(root, name)?,
    ))
}

fn string_property(value: &str) -> Result<windows_core::IInspectable> {
    windows::Foundation::PropertyValue::CreateString(&HSTRING::from(value))
        .map_err(|error| naui::Error::new("create a WinUI string property", error.to_string()))
}

fn toolbar(tasks: Tasks, root: &FrameworkElement, open_pr_title_button: Button) -> Result<Toolbar> {
    Ok(Toolbar::from_controls(vec![
        Some(open_pr_title_button),
        Some(app_bar_button(tasks.clone(), root, "RefreshReviewButton")?),
        None,
        Some(app_bar_button(tasks.clone(), root, "PreviousChangeButton")?),
        Some(app_bar_button(tasks.clone(), root, "NextChangeButton")?),
        Some(button(tasks.clone(), root, "FocusModeButton")?),
        None,
        Some(app_bar_button(tasks.clone(), root, "MarkReviewedButton")?),
        None,
        Some(button(tasks, root, "DescriptionButton")?),
    ]))
}

fn sidebar(ui: &Ui, root: &FrameworkElement) -> Result<Sidebar> {
    let resources = Resources::en_us();
    let native = raw::element::<NavigationView>(root, "ReviewNavigationView")?;
    native
        .SetPaneTitle(&HSTRING::from(resource(resources, "Navigation.PaneTitle")?))
        .map_err(|error| {
            naui::Error::new("set the WinUI navigation pane title", error.to_string())
        })?;
    raw::set_accessible_label_on(&native, resource(resources, "Navigation.PaneName")?);
    let reviewed_list = raw::element::<ListView>(root, "ReviewedReviewsList")?;
    let reviewed_panel = raw::element::<XamlStackPanel>(root, "ReviewedReviewsPanel")?;
    let reviewed_expander = raw::element::<Expander>(root, "ReviewedReviewsExpander")?;
    let reviewed_header = string_property(resource(resources, "Navigation.Reviewed")?)?;
    reviewed_expander
        .SetHeader(&reviewed_header)
        .map_err(|error| {
            naui::Error::new("set the reviewed pull requests header", error.to_string())
        })?;
    raw::set_accessible_label_on(
        &reviewed_expander,
        resource(resources, "Navigation.ReviewedName")?,
    );
    let activity = button(ui.tasks(), root, "OpenReviewActivityButton")?;
    activity.set_accessible_label(resource(resources, "App.ActivityName")?);
    activity.set_tooltip(resource(resources, "App.ActivityTooltip")?);
    let activity_label = raw::element::<XamlTextBlock>(root, "OpenReviewActivityLabel")?;
    let sidebar = Sidebar::from_native(
        ui.tasks(),
        native,
        reviewed_list,
        reviewed_panel,
        activity,
        activity_label,
    );
    sidebar.set_width(SIDEBAR_WIDTH);
    Ok(sidebar)
}

fn initialize_shell_strings(
    tasks: Tasks,
    root: &FrameworkElement,
    resources: Resources,
) -> Result<()> {
    label(root, "OpenReviewActivityLabel")?.set_text(resource(resources, "App.Activity")?);
    label(root, "DescriptionLabel")?.set_text(resource(resources, "Review.Description")?);
    label(root, "EmptyReviewTitle")?.set_text(resource(resources, "Review.EmptyTitle")?);
    label(root, "EmptyReviewDescription")?.set_text(resource(resources, "App.EmptyHint")?);
    label(root, "ChangedFilesSummary")?.set_text(resource(resources, "Review.Files")?);
    label(root, "OverviewHeader")?.set_text(resource(resources, "Review.Overview")?);
    label(root, "NotesHeader")?.set_text(resource(resources, "Review.NotesHeader")?);
    label(root, "CommentsHeader")?.set_text(resource(resources, "Review.LineComments")?);
    label(root, "ExistingCommentsHeader")?
        .set_text(resource(resources, "Review.ExistingComments")?);
    label(root, "SelectedCodeHeader")?.set_text(resource(resources, "Review.SelectedCode")?);
    label(root, "EmptyCommentsText")?.set_text(resource(resources, "Review.EmptyComments")?);
    label(root, "NewIterationPill")?.set_text(resource(resources, "Review.NewIteration")?);
    label(root, "ActivityTitle")?.set_text(resource(resources, "Activity.Title")?);
    label(root, "ActivityPullRequestLabel")?
        .set_text(resource(resources, "Activity.PullRequestTotalLabel")?);
    label(root, "ActivityCommentLabel")?
        .set_text(resource(resources, "Activity.CommentTotalLabel")?);
    label(root, "ActivityReviewLabel")?.set_text(resource(resources, "Activity.ReviewTotalLabel")?);
    label(root, "ActivityChartLegendPullRequests")?
        .set_text(resource(resources, "Activity.PullRequests")?);
    label(root, "ActivityChartLegendComments")?.set_text(resource(resources, "Activity.Comments")?);
    label(root, "ActivityChartLegendReviews")?.set_text(resource(resources, "Activity.Reviews")?);
    label(root, "RecentReviewsHeader")?.set_text(resource(resources, "Activity.RecentReviews")?);
    label(root, "RecentReviewsEmpty")?.set_text(resource(resources, "Activity.Empty")?);

    for (name, key) in [
        ("OpenReviewButton", "OpenReview.Open"),
        ("CancelOpenReviewButton", "OpenReview.Cancel"),
        ("AuthenticateButton", "AuthenticateButton.Content"),
        ("QueueCommentButton", "Review.SaveComment"),
        ("PublishCommentsButton", "Review.NoCommentsToSubmit"),
        ("SaveNotesButton", "Notes.Save"),
        ("BackToDiffButton", "Review.BackToDiff"),
        ("MarkdownToggleButton", "Review.MarkdownToggle"),
    ] {
        let native = raw::element::<XamlButton>(root, name)?;
        let value = string_property(resource(resources, key)?)?;
        native
            .SetContent(&value)
            .map_err(|error| naui::Error::new("set a WinUI button label", error.to_string()))?;
    }
    for (name, key) in [
        ("CommentsInspectorTab", "Review.Comments"),
        ("OverviewInspectorTab", "Review.Overview"),
        ("NotesInspectorTab", "Review.Notes"),
        ("CurrentFileCommentsFilter", "Review.CurrentFile"),
        ("HideBotCommentsFilter", "Review.HideBots"),
    ] {
        let value = string_property(resource(resources, key)?)?;
        raw::element::<XamlToggleButton>(root, name)?
            .SetContent(&value)
            .map_err(|error| naui::Error::new("set a WinUI toggle label", error.to_string()))?;
    }
    for (name, label_key, name_key, tooltip_key) in [
        (
            "RefreshReviewButton",
            "Review.Refresh",
            "Review.Refresh",
            "Review.Refresh",
        ),
        (
            "PreviousChangeButton",
            "Review.PreviousChange",
            "Review.PreviousChangeName",
            "Review.PreviousChangeTooltip",
        ),
        (
            "NextChangeButton",
            "Review.NextChange",
            "Review.NextChangeName",
            "Review.NextChangeTooltip",
        ),
        (
            "PreviousSearchResultButton",
            "Review.PreviousResult",
            "Review.PreviousResult",
            "Review.PreviousResultTooltip",
        ),
        (
            "NextSearchResultButton",
            "Review.NextResult",
            "Review.NextResult",
            "Review.NextResultTooltip",
        ),
        (
            "CollapseFunctionsButton",
            "Review.CollapseFunctions",
            "Review.CollapseFunctions",
            "Review.CollapseFunctions",
        ),
        (
            "ExpandFunctionsButton",
            "Review.ExpandFunctions",
            "Review.ExpandFunctions",
            "Review.ExpandFunctions",
        ),
        (
            "MarkReviewedButton",
            "Review.MarkReviewed",
            "Review.MarkReviewedName",
            "Review.MarkReviewed",
        ),
    ] {
        let control = app_bar_button(tasks.clone(), root, name)?;
        control.set_text(resource(resources, label_key)?);
        control.set_accessible_label(resource(resources, name_key)?);
        control.set_tooltip(resource(resources, tooltip_key)?);
    }
    for (name, a11y_key, tooltip_key) in [
        ("FocusModeButton", "App.FocusModeName", "App.Focus"),
        (
            "DescriptionButton",
            "Review.DescriptionName",
            "Review.DescriptionTooltip",
        ),
        (
            "ReviewActionsButton",
            "Review.MoreActions",
            "Review.MoreActions",
        ),
        (
            "MarkdownToggleButton",
            "Review.MarkdownToggleName",
            "Review.MarkdownToggleTooltip",
        ),
        (
            "AuthenticateButton",
            "AuthenticateButton.Name",
            "AuthenticateButton.Content",
        ),
        (
            "CollapseAllCommentsButton",
            "Review.CollapseComments",
            "Review.CollapseComments",
        ),
        (
            "ExpandAllCommentsButton",
            "Review.ExpandComments",
            "Review.ExpandComments",
        ),
        ("ActivityBackButton", "Activity.BackName", "App.Back"),
        (
            "ActivityPreviousMonthButton",
            "Activity.PreviousMonthName",
            "Activity.PreviousMonth",
        ),
        (
            "ActivityNextMonthButton",
            "Activity.NextMonthName",
            "Activity.NextMonth",
        ),
    ] {
        let control = button(tasks.clone(), root, name)?;
        control.set_accessible_label(resource(resources, a11y_key)?);
        control.set_tooltip(resource(resources, tooltip_key)?);
    }
    Ok(())
}

struct StatusPane {
    root: Pane,
    auth_bar: InfoBar,
    error_bar: InfoBar,
    label: Label,
    authenticate: Button,
}

impl StatusPane {
    fn new(tasks: Tasks, root: &FrameworkElement) -> Result<Self> {
        let resources = Resources::en_us();
        let auth_bar = info_bar(tasks.clone(), root, "AuthenticationInfoBar")?;
        auth_bar.set_title(resource(resources, "AuthenticationInfoBar.Title")?);
        let error_bar = info_bar(tasks.clone(), root, "GlobalErrorInfoBar")?;
        Ok(Self {
            root: pane(root, "ReviewStatusPanel")?,
            auth_bar,
            error_bar,
            label: label(root, "GlobalStatusText")?,
            authenticate: button(tasks, root, "AuthenticateButton")?,
        })
    }
}

struct OpenReviewSheet {
    panel: Pane,
    sheet: RawSheet,
    progress: XamlProgressRing,
    dialog_progress: XamlProgressRing,
    url: TextInput,
    repository: TextInput,
    browse: FilePicker,
    browse_button: Button,
    inline_url: TextInput,
    inline_repository: TextInput,
    inline_browse: FilePicker,
    inline_browse_button: Button,
    inline_open: Button,
    inline_cancel: Button,
}

impl OpenReviewSheet {
    fn new(ui: &Ui, tasks: Tasks, root: &FrameworkElement, window: XamlWindow) -> Result<Self> {
        let resources = Resources::en_us();
        let dialog = raw::element::<ContentDialog>(root, "OpenReviewDialog")?;
        let title = string_property(resource(resources, "OpenReview.Title")?)?;
        let _ = dialog.SetTitle(&title);
        let _ =
            dialog.SetPrimaryButtonText(&HSTRING::from(resource(resources, "OpenReview.Open")?));
        let _ =
            dialog.SetCloseButtonText(&HSTRING::from(resource(resources, "OpenReview.Cancel")?));
        let _ = dialog.SetIsPrimaryButtonEnabled(false);
        let browse = ui.file_picker(resource(resources, "OpenReview.Browse")?)?;
        browse.set_mode(FilePickerMode::Folder);
        let inline_browse = ui.file_picker(resource(resources, "OpenReview.Browse")?)?;
        inline_browse.set_mode(FilePickerMode::Folder);
        let browse_button = button(tasks.clone(), root, "OpenReviewDialogBrowseButton")?;
        browse_button.set_accessible_label(resource(resources, "OpenReview.BrowseName")?);
        browse_button.set_tooltip(resource(resources, "OpenReview.BrowseTooltip")?);
        let inline_browse_button = button(tasks.clone(), root, "BrowseRepositoryButton")?;
        inline_browse_button.set_accessible_label(resource(resources, "OpenReview.BrowseName")?);
        inline_browse_button.set_tooltip(resource(resources, "OpenReview.BrowseTooltip")?);
        let url = text_input(tasks.clone(), root, "OpenReviewDialogUrlTextBox")?;
        url.set_header(resource(resources, "OpenReview.UrlLabel")?);
        url.set_placeholder(resource(resources, "OpenReview.UrlPlaceholder")?);
        url.set_accessible_label(resource(resources, "OpenReview.UrlName")?);
        let repository = text_input(tasks.clone(), root, "OpenReviewDialogRepositoryTextBox")?;
        repository.set_header(resource(resources, "OpenReview.RepositoryLabel")?);
        repository.set_placeholder(resource(resources, "OpenReview.RepositoryPlaceholder")?);
        repository.set_accessible_label(resource(resources, "OpenReview.RepositoryName")?);
        let inline_url = text_input(tasks.clone(), root, "PullRequestUrlTextBox")?;
        inline_url.set_header(resource(resources, "OpenReview.UrlLabel")?);
        inline_url.set_placeholder(resource(resources, "OpenReview.UrlPlaceholder")?);
        inline_url.set_accessible_label(resource(resources, "OpenReview.UrlName")?);
        let inline_repository = text_input(tasks.clone(), root, "LocalRepositoryTextBox")?;
        inline_repository.set_header(resource(resources, "OpenReview.RepositoryLabel")?);
        inline_repository.set_placeholder(resource(resources, "OpenReview.RepositoryPlaceholder")?);
        inline_repository.set_accessible_label(resource(resources, "OpenReview.RepositoryName")?);
        Ok(Self {
            panel: pane(root, "OpenReviewPanel")?,
            sheet: RawSheet::from_native(tasks.clone(), window, dialog),
            progress: raw::element::<XamlProgressRing>(root, "OpenProgressRing")?,
            dialog_progress: raw::element::<XamlProgressRing>(
                root,
                "OpenReviewDialogProgressRing",
            )?,
            url,
            repository,
            browse,
            browse_button,
            inline_url,
            inline_repository,
            inline_browse,
            inline_browse_button,
            inline_open: button(tasks.clone(), root, "OpenReviewButton")?,
            inline_cancel: button(tasks, root, "CancelOpenReviewButton")?,
        })
    }
}

struct ActivityPane {
    root: Pane,
    back: Button,
    previous_month: Button,
    next_month: Button,
    progress: XamlProgressRing,
    error: InfoBar,
    month: Label,
    pull_request_total: Label,
    comment_total: Label,
    review_total: Label,
    chart: Canvas,
    recent: Table,
    recent_host: Pane,
    empty: Label,
}

impl ActivityPane {
    fn new(ui: &Ui, tasks: Tasks, root: &FrameworkElement) -> Result<Self> {
        let resources = Resources::en_us();
        let chart = ui.canvas()?;
        chart.set_sizing(Sizing::new().width(Length::Fill).height(Length::Fill));
        native::host_naui_widget(
            &raw::element::<XamlGrid>(root, "ActivityChartHost")?,
            &chart,
        )?;
        let recent = ui.table()?;
        recent.set_columns(&[
            TableColumn::new(resource(resources, "Activity.Review")?).width(320.0),
            TableColumn::new(resource(resources, "Activity.Repository")?).width(240.0),
            TableColumn::new(resource(resources, "Activity.Date")?).width(140.0),
        ]);
        recent.set_sizing(Sizing::fill());
        let recent_host_grid = raw::element::<XamlGrid>(root, "RecentReviewsHost")?;
        native::host_naui_widget(&recent_host_grid, &recent)?;
        Ok(Self {
            root: pane(root, "ActivityHost")?,
            back: button(tasks.clone(), root, "ActivityBackButton")?,
            previous_month: button(tasks.clone(), root, "ActivityPreviousMonthButton")?,
            next_month: button(tasks.clone(), root, "ActivityNextMonthButton")?,
            progress: raw::element::<XamlProgressRing>(root, "ActivityProgressRing")?,
            error: info_bar(tasks, root, "ActivityErrorInfoBar")?,
            month: label(root, "ActivityMonthLabel")?,
            pull_request_total: label(root, "ActivityPullRequestTotal")?,
            comment_total: label(root, "ActivityCommentTotal")?,
            review_total: label(root, "ActivityReviewTotal")?,
            chart,
            recent,
            recent_host: Pane::from_element(raw::ui_element(&recent_host_grid)?),
            empty: label(root, "RecentReviewsEmpty")?,
        })
    }
}

struct DescriptionSheet {
    sheet: RawSheet,
    content: raw::Stack,
}

impl DescriptionSheet {
    fn new(tasks: Tasks, window: XamlWindow, root: &FrameworkElement) -> Result<Self> {
        let resources = Resources::en_us();
        let dialog = raw::element::<ContentDialog>(root, "DescriptionDialog")?;
        let title = string_property(resource(resources, "Review.DescriptionDialogTitle")?)?;
        let _ = dialog.SetTitle(&title);
        let _ = dialog.SetCloseButtonText(&HSTRING::from(resource(resources, "Review.Close")?));
        Ok(Self {
            sheet: RawSheet::from_native(tasks, window, dialog),
            content: raw::Stack::from_native(raw::element::<XamlStackPanel>(
                root,
                "DescriptionDialogContent",
            )?),
        })
    }
}

struct CompletionSheet {
    sheet: RawSheet,
}

impl CompletionSheet {
    fn new(tasks: Tasks, window: XamlWindow, root: &FrameworkElement) -> Result<Self> {
        let resources = Resources::en_us();
        let dialog = raw::element::<ContentDialog>(root, "CompletionDialog")?;
        let title = string_property(resource(resources, "Review.CompleteTitle")?)?;
        let _ = dialog.SetTitle(&title);
        let _ =
            dialog.SetPrimaryButtonText(&HSTRING::from(resource(resources, "Review.Complete")?));
        let _ = dialog.SetCloseButtonText(&HSTRING::from(resource(resources, "Review.Cancel")?));
        label(root, "CompletionDialogMessage")?
            .set_text(resource(resources, "Review.CompleteConfirm")?);
        Ok(Self {
            sheet: RawSheet::from_native(tasks, window, dialog),
        })
    }
}

struct DeletionSheet {
    sheet: RawSheet,
}

impl DeletionSheet {
    fn new(tasks: Tasks, window: XamlWindow, root: &FrameworkElement) -> Result<Self> {
        let resources = Resources::en_us();
        let dialog = raw::element::<ContentDialog>(root, "DeletionDialog")?;
        let title = string_property(resource(resources, "Review.DeleteTitle")?)?;
        let _ = dialog.SetTitle(&title);
        let _ = dialog.SetPrimaryButtonText(&HSTRING::from(resource(resources, "Review.Delete")?));
        let _ = dialog.SetCloseButtonText(&HSTRING::from(resource(resources, "Review.Cancel")?));
        label(root, "DeletionDialogMessage")?
            .set_text(resource(resources, "Review.DeleteConfirm")?);
        Ok(Self {
            sheet: RawSheet::from_native(tasks, window, dialog),
        })
    }
}

struct FocusedCommentSheet {
    sheet: RawSheet,
    line_header: Label,
    context: Label,
    comment: TextInput,
    suggestion: TextInput,
}

impl FocusedCommentSheet {
    fn new(tasks: Tasks, window: XamlWindow, root: &FrameworkElement) -> Result<Self> {
        let resources = Resources::en_us();
        let dialog = raw::element::<ContentDialog>(root, "FocusedCommentDialog")?;
        let title = string_property(resource(resources, "Review.AddCommentTitle")?)?;
        let _ = dialog.SetTitle(&title);
        let _ =
            dialog.SetPrimaryButtonText(&HSTRING::from(resource(resources, "Review.SaveLocally")?));
        let _ = dialog.SetCloseButtonText(&HSTRING::from(resource(resources, "Review.Cancel")?));
        let comment = text_input(tasks.clone(), root, "FocusedCommentTextBox")?;
        comment.set_header(resource(resources, "Review.EditCommentHeader")?);
        comment.set_placeholder(resource(resources, "Review.FocusedCommentPlaceholder")?);
        comment.set_accessible_label(resource(resources, "Review.FocusedCommentName")?);
        let suggestion = text_input(tasks.clone(), root, "FocusedSuggestionTextBox")?;
        suggestion.set_header(resource(resources, "Review.SuggestionHeader")?);
        suggestion.set_placeholder(resource(resources, "Review.FocusedSuggestionPlaceholder")?);
        suggestion.set_accessible_label(resource(resources, "Review.SuggestionName")?);
        Ok(Self {
            sheet: RawSheet::from_native(tasks, window, dialog),
            line_header: label(root, "FocusedCommentLineHeader")?,
            context: label(root, "FocusedCommentContext")?,
            comment,
            suggestion,
        })
    }
}

struct ReviewWorkspace {
    root: Pane,
    files_column: ColumnDefinition,
    files_splitter_column: ColumnDefinition,
    inspector_splitter_column: ColumnDefinition,
    inspector_column: ColumnDefinition,
    files_splitter: PaneSplitter,
    inspector_splitter: PaneSplitter,
    title: Label,
    subtitle: Label,
    workspace_status: Label,
    new_iteration_badge: Pane,
    focus_button: Button,
    focus_glyph: FontIcon,
    progress: ProgressBar,
    iteration: ComboBox,
    archive_menu: MenuItem,
    reopen_menu: MenuItem,
    actions_separator: Pane,
    complete_menu: MenuItem,
    files: FilePane,
    diff: DiffPane,
    inspector: InspectorPane,
}

impl ReviewWorkspace {
    fn new(ui: &Ui, tasks: Tasks, root: &FrameworkElement) -> Result<Self> {
        let resources = Resources::en_us();
        let files = FilePane::new(ui, tasks.clone(), root)?;
        let diff = DiffPane::new(ui, tasks.clone(), root)?;
        let inspector = InspectorPane::new(tasks.clone(), root)?;
        let iteration = ComboBox::from_native(
            tasks.clone(),
            raw::element::<XamlComboBox>(root, "IterationComboBox")?,
        );
        iteration.set_accessible_label(resource(resources, "Review.IterationName")?);
        iteration.set_tooltip(resource(resources, "Review.IterationTooltip")?);

        let files_splitter = PaneSplitter::from_native(
            tasks.clone(),
            raw::element::<ContentControl>(root, "ChangedFilesPaneSplitter")?,
        );
        files_splitter.set_accessible_label(resource(resources, "Splitter.Files")?);
        let inspector_splitter = PaneSplitter::from_native(
            tasks.clone(),
            raw::element::<ContentControl>(root, "ReviewInspectorPaneSplitter")?,
        );
        inspector_splitter.set_accessible_label(resource(resources, "Splitter.Inspector")?);

        let archive_menu = menu_item(tasks.clone(), root, "ArchiveReviewMenuItem")?;
        archive_menu.set_text(resource(resources, "Review.Archive")?);
        let reopen_menu = menu_item(tasks.clone(), root, "ReopenReviewMenuItem")?;
        reopen_menu.set_text(resource(resources, "Review.Reopen")?);
        let complete_menu = menu_item(tasks.clone(), root, "CompleteReviewMenuItem")?;
        complete_menu.set_text(resource(resources, "Review.CompleteMenuItem")?);

        Ok(Self {
            root: pane(root, "ActiveReviewGrid")?,
            files_column: raw::element::<ColumnDefinition>(root, "ChangedFilesColumn")?,
            files_splitter_column: raw::element::<ColumnDefinition>(
                root,
                "ChangedFilesSplitterColumn",
            )?,
            inspector_splitter_column: raw::element::<ColumnDefinition>(
                root,
                "InspectorSplitterColumn",
            )?,
            inspector_column: raw::element::<ColumnDefinition>(root, "InspectorColumn")?,
            files_splitter,
            inspector_splitter,
            title: label(root, "ReviewTitle")?,
            subtitle: label(root, "ReviewWorkspaceSubtitle")?,
            workspace_status: label(root, "ReviewWorkspaceStatus")?,
            new_iteration_badge: pane(root, "NewIterationBadge")?,
            focus_button: button(tasks, root, "FocusModeButton")?,
            focus_glyph: raw::element::<FontIcon>(root, "FocusModeGlyph")?,
            progress: ProgressBar::from_native(raw::element::<XamlProgressBar>(
                root,
                "ReviewProgressBar",
            )?),
            iteration,
            archive_menu,
            reopen_menu,
            actions_separator: pane(root, "ReviewActionsSeparator")?,
            complete_menu,
            files,
            diff,
            inspector,
        })
    }
}

struct FilePane {
    root: Pane,
    summary: Label,
    search: TextInput,
    tree: Tree,
}

impl FilePane {
    fn new(ui: &Ui, tasks: Tasks, root: &FrameworkElement) -> Result<Self> {
        let resources = Resources::en_us();
        let tree = ui.tree()?;
        tree.set_sizing(Sizing::fill());
        let tree_host = raw::element::<XamlGrid>(root, "FileTreeHost")?;
        native::host_naui_widget(&tree_host, &tree)?;
        raw::set_accessible_label_on(&tree_host, resource(resources, "Review.FilesTreeName")?);
        let search = text_input(tasks, root, "ChangedFileFilterTextBox")?;
        search.set_placeholder(resource(resources, "Review.FileFilterPlaceholder")?);
        search.set_accessible_label(resource(resources, "Review.FileFilterName")?);
        Ok(Self {
            root: pane(root, "ChangedFilesPane")?,
            summary: label(root, "ChangedFilesSummary")?,
            search,
            tree,
        })
    }
}

struct DiffPane {
    search: TextInput,
    search_position: Label,
    previous_search: Button,
    next_search: Button,
    layout: ComboBox,
    context_container: Pane,
    context: ComboBox,
    collapse: Button,
    expand: Button,
    functions_separator: Pane,
    markdown_container: Pane,
    markdown_toggle: Button,
    back_to_diff: Button,
    status: Label,
    marker_column: ColumnDefinition,
    marker_host: Pane,
    marker: Canvas,
    raw: Pane,
    table: DiffList,
    markdown_scroll: Pane,
    markdown_content: raw::Stack,
}

impl DiffPane {
    fn new(ui: &Ui, tasks: Tasks, root: &FrameworkElement) -> Result<Self> {
        let resources = Resources::en_us();
        let table = DiffList::new(ui, raw::element::<XamlGrid>(root, "DiffListHost")?)?;
        let marker = ui.canvas()?;
        marker.set_sizing(Sizing::fill());
        let marker_grid = raw::element::<XamlGrid>(root, "DiffMarkerHost")?;
        native::host_naui_widget(&marker_grid, &marker)?;
        raw::set_accessible_label_on(&marker_grid, resource(resources, "Diff.MarkersName")?);
        raw::set_tooltip_on(&marker_grid, resource(resources, "Diff.MarkersTooltip")?);
        let layout = ComboBox::from_native(
            tasks.clone(),
            raw::element::<XamlComboBox>(root, "DiffLayoutComboBox")?,
        );
        layout.set_items(&[
            resource(resources, "Review.Diff")?,
            resource(resources, "Review.SideBySide")?,
        ]);
        layout.set_accessible_label(resource(resources, "Review.DiffLayoutName")?);
        layout.set_tooltip(resource(resources, "Review.DiffLayoutTooltip")?);
        let context = ComboBox::from_native(
            tasks.clone(),
            raw::element::<XamlComboBox>(root, "DiffContextComboBox")?,
        );
        context.set_items(&["3", "5", "10", "20", "40"]);
        context.set_accessible_label(resource(resources, "Review.DiffContextName")?);
        context.set_tooltip(resource(resources, "Review.DiffContextTooltip")?);
        let search = text_input(tasks.clone(), root, "FileSearchTextBox")?;
        search.set_placeholder(resource(resources, "Review.SearchCode")?);
        search.set_accessible_label(resource(resources, "Review.SearchName")?);
        search.set_tooltip(resource(resources, "Review.SearchTooltip")?);
        Ok(Self {
            search,
            search_position: label(root, "FileSearchPositionText")?,
            previous_search: app_bar_button(tasks.clone(), root, "PreviousSearchResultButton")?,
            next_search: app_bar_button(tasks.clone(), root, "NextSearchResultButton")?,
            layout,
            context_container: pane(root, "DiffContextContainer")?,
            context,
            collapse: app_bar_button(tasks.clone(), root, "CollapseFunctionsButton")?,
            expand: app_bar_button(tasks.clone(), root, "ExpandFunctionsButton")?,
            functions_separator: pane(root, "FunctionsSeparator")?,
            markdown_container: pane(root, "MarkdownToggleContainer")?,
            markdown_toggle: button(tasks.clone(), root, "MarkdownToggleButton")?,
            back_to_diff: button(tasks, root, "BackToDiffButton")?,
            status: label(root, "ReviewStatusText")?,
            marker_column: raw::element::<ColumnDefinition>(root, "DiffMarkerColumn")?,
            marker_host: Pane::from_element(raw::ui_element(&marker_grid)?),
            marker,
            raw: pane(root, "RawDiffHost")?,
            table,
            markdown_scroll: pane(root, "FormattedMarkdownViewer")?,
            markdown_content: raw::Stack::from_native(raw::element::<XamlStackPanel>(
                root,
                "FormattedMarkdownContent",
            )?),
        })
    }
}

struct InspectorPane {
    root: Pane,
    tabs: Tabs,
    error: InfoBar,
    overview_provider: Label,
    overview_link: Hyperlink,
    overview_repo_source: Label,
    overview_repo_location: Label,
    comments: CommentsPane,
    notes: TextInput,
    notes_status: Label,
    save_notes: Button,
}

impl InspectorPane {
    fn new(tasks: Tasks, root: &FrameworkElement) -> Result<Self> {
        let resources = Resources::en_us();
        let tabs = Tabs::new(
            [
                toggle_button(tasks.clone(), root, "CommentsInspectorTab")?,
                toggle_button(tasks.clone(), root, "OverviewInspectorTab")?,
                toggle_button(tasks.clone(), root, "NotesInspectorTab")?,
            ],
            [
                pane(root, "CommentsPanel")?,
                pane(root, "OverviewPanel")?,
                pane(root, "NotesPanel")?,
            ],
        );
        let notes = text_input(tasks.clone(), root, "NotesTextBox")?;
        notes.set_placeholder(resource(resources, "Review.NotesPlaceholder")?);
        notes.set_accessible_label(resource(resources, "Review.NotesName")?);
        let overview_link = Hyperlink::from_native(
            tasks.clone(),
            raw::element::<HyperlinkButton>(root, "ReviewHyperlink")?,
        );
        Ok(Self {
            root: pane(root, "ReviewInspectorPane")?,
            tabs,
            error: info_bar(tasks.clone(), root, "ReviewErrorInfoBar")?,
            overview_provider: label(root, "ProviderLabel")?,
            overview_link,
            overview_repo_source: label(root, "RepositorySourceLabel")?,
            overview_repo_location: label(root, "RepositoryLocationLabel")?,
            comments: CommentsPane::new(tasks.clone(), root)?,
            notes,
            notes_status: label(root, "NotesSaveStatus")?,
            save_notes: button(tasks, root, "SaveNotesButton")?,
        })
    }
}

struct CommentsPane {
    collapse_all: Button,
    expand_all: Button,
    current_file: Button,
    hide_bots: Button,
    selected_line: Label,
    selected_code_panel: Pane,
    selected_code: Label,
    draft: TextInput,
    suggestion: TextInput,
    save_comment: Button,
    pending_summary: Label,
    publish: Button,
    list: raw::Stack,
    remote_panel: Pane,
    remote_list: raw::Stack,
    empty: Label,
}

impl CommentsPane {
    fn new(tasks: Tasks, root: &FrameworkElement) -> Result<Self> {
        let resources = Resources::en_us();
        let draft = text_input(tasks.clone(), root, "DraftCommentTextBox")?;
        draft.set_placeholder(resource(resources, "Review.CommentPlaceholder")?);
        draft.set_accessible_label(resource(resources, "Review.CommentName")?);
        let suggestion = text_input(tasks.clone(), root, "DraftSuggestionTextBox")?;
        suggestion.set_header(resource(resources, "Review.SuggestionHeader")?);
        suggestion.set_placeholder(resource(resources, "Review.SuggestionPlaceholder")?);
        suggestion.set_accessible_label(resource(resources, "Review.SuggestionName")?);
        Ok(Self {
            collapse_all: button(tasks.clone(), root, "CollapseAllCommentsButton")?,
            expand_all: button(tasks.clone(), root, "ExpandAllCommentsButton")?,
            current_file: toggle_button(tasks.clone(), root, "CurrentFileCommentsFilter")?,
            hide_bots: toggle_button(tasks.clone(), root, "HideBotCommentsFilter")?,
            selected_line: label(root, "SelectedLineDescription")?,
            selected_code_panel: pane(root, "SelectedCodePanel")?,
            selected_code: label(root, "SelectedCodeFragment")?,
            draft,
            suggestion,
            save_comment: button(tasks.clone(), root, "QueueCommentButton")?,
            pending_summary: label(root, "PendingCommentSummary")?,
            publish: button(tasks, root, "PublishCommentsButton")?,
            list: raw::Stack::from_native(raw::element::<XamlStackPanel>(
                root,
                "LocalCommentsList",
            )?),
            remote_panel: pane(root, "RemoteCommentsPanel")?,
            remote_list: raw::Stack::from_native(raw::element::<XamlStackPanel>(
                root,
                "RemoteCommentsList",
            )?),
            empty: label(root, "EmptyCommentsText")?,
        })
    }
}
