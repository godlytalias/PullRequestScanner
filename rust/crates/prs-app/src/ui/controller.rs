use super::*;
use crate::service::{
    AppServices, CommentPublishResult, ReviewSessionService, ReviewSessionServiceError,
};
use crate::state::{
    ActivityMonth, DiffHighlightKind, DiffLayout, DiffLineState, FileTreeNodeState, MainAppState,
    NotesSaveState, ReviewActivityPageState, ReviewNavigationStatus, ReviewSessionAppState,
};
use naui::{
    CloseResponse, DialogResponse, Key, ListRow, Painter, Point, Rect, RichText, Sender, Shortcut,
    TableRow, TextColor, TextRun, Timer, TreeItem,
};
use prs_core::{
    DiffBlock, DiffLineKind, MarkdownBlock, MarkdownBlockKind, MarkdownInlineKind,
    PullRequestReference, RemoteReviewComment, ReviewActivityDay, ReviewActivityRecord,
    ReviewCommentSide, ReviewCommentStatus, ReviewSessionState,
};
use prs_winui_projection::Microsoft::UI::Xaml::Controls::{
    Button as XamlButton, ColumnDefinition, FontIcon, Grid as XamlGrid,
    StackPanel as XamlStackPanel, TextBlock,
};
use prs_winui_projection::Microsoft::UI::Xaml::{
    FrameworkElement, GridLength, GridUnitType, RoutedEventHandler, UIElement,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;
use windows_core::{HSTRING, Interface};

const REVIEW_REFRESH_INTERVAL: Duration = Duration::from_secs(300);
const NOTES_SAVE_DELAY: Duration = Duration::from_millis(500);
const DIFF_LINE_XAML: &str = include_str!("../../resources/DiffLine.xaml");
const SIDE_BY_SIDE_ROW_XAML: &str = include_str!("../../resources/SideBySideRow.xaml");
const COMMENT_CARD_XAML: &str = include_str!("../../resources/CommentCard.xaml");
const COMMENT_REPLY_XAML: &str = include_str!("../../resources/CommentReply.xaml");

type UiEvent = Box<dyn FnOnce(&AppController) + Send + 'static>;
type CodeSelectionEvent = (usize, String);

pub fn build(ui: &Ui) -> Result<()> {
    let services = AppServices::from_local_app_data()
        .map_err(|error| naui::Error::new("initialize the review services", error.to_string()))?;
    let app = AppController::new(ui.clone(), services)?;
    app.start();
    std::mem::forget(app);
    Ok(())
}

#[derive(Debug, Clone)]
struct UiFailure {
    message: String,
    authentication_reference: Option<PullRequestReference>,
}

impl From<ReviewSessionServiceError> for UiFailure {
    fn from(error: ReviewSessionServiceError) -> Self {
        Self {
            message: error.to_string(),
            authentication_reference: error.authentication_reference().cloned(),
        }
    }
}

#[derive(Debug, Clone)]
enum PendingAction {
    OpenReview {
        url: String,
        repository_path: Option<PathBuf>,
    },
    RefreshReview {
        session_key: String,
    },
    SelectIteration {
        session_key: String,
        iteration_id: Option<String>,
    },
    LoadDiff {
        session_key: String,
        path: String,
        context_lines: i32,
        complete_file: bool,
    },
    PublishComments {
        session_key: String,
    },
    ReplyToComment {
        session_key: String,
        comment: Box<RemoteReviewComment>,
        body: String,
    },
    DeleteComment {
        session_key: String,
        comment_id: String,
    },
}

#[derive(Default)]
struct ControllerState {
    main: MainAppState,
    reviews: HashMap<String, ReviewSessionAppState>,
    activity: ReviewActivityPageState,
    authentication_reference: Option<PullRequestReference>,
    pending_action: Option<PendingAction>,
    pending_delete_comment: Option<String>,
    pending_comment_jump: Option<PendingLineJump>,
    close_after_save: bool,
    pending_close_saves: usize,
}

#[derive(Debug, Clone)]
struct PendingLineJump {
    session_key: String,
    path: String,
    line_number: Option<usize>,
    side: Option<ReviewCommentSide>,
}

impl ControllerState {
    fn active_review(&self) -> Option<&ReviewSessionAppState> {
        let key = self.main.active_review_key.as_deref()?;
        self.reviews.get(key)
    }

    fn active_review_mut(&mut self) -> Option<&mut ReviewSessionAppState> {
        let key = self.main.active_review_key.clone()?;
        self.reviews.get_mut(&key)
    }
}

struct AppController {
    ui: Ui,
    view: AppShell,
    services: ReviewSessionService,
    runtime: Arc<tokio::runtime::Runtime>,
    sender: Sender<UiEvent>,
    selection_sender: Sender<CodeSelectionEvent>,
    state: RefCell<ControllerState>,
    refresh_timer: RefCell<Option<Timer>>,
    notes_timers: RefCell<HashMap<String, Timer>>,
    refresh_generation: Cell<u64>,
    self_weak: RefCell<Weak<AppController>>,
}

impl AppController {
    fn new(ui: Ui, services: AppServices) -> Result<Rc<Self>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("prs-background")
            .build()
            .map_err(|error| {
                naui::Error::new("initialize the asynchronous runtime", error.to_string())
            })?;
        let view = AppShell::new(&ui)?;
        let weak_slot = Rc::new(RefCell::new(None::<Weak<AppController>>));
        let sender = ui.tasks().channel({
            let weak_slot = weak_slot.clone();
            move |event: UiEvent| {
                let controller = weak_slot.borrow().as_ref().and_then(Weak::upgrade);
                if let Some(controller) = controller {
                    event(&controller);
                }
            }
        });
        let selection_sender = ui.tasks().channel({
            let weak_slot = weak_slot.clone();
            move |(source_index, selected): CodeSelectionEvent| {
                let controller = weak_slot.borrow().as_ref().and_then(Weak::upgrade);
                if let Some(controller) = controller {
                    controller.apply_code_selection(source_index, selected);
                }
            }
        });
        let services = services.review_sessions().clone();
        let app = Rc::new(Self {
            ui,
            view,
            services,
            runtime: Arc::new(runtime),
            sender,
            selection_sender,
            state: RefCell::new(ControllerState::default()),
            refresh_timer: RefCell::new(None),
            notes_timers: RefCell::new(HashMap::new()),
            refresh_generation: Cell::new(0),
            self_weak: RefCell::new(Weak::new()),
        });
        *weak_slot.borrow_mut() = Some(Rc::downgrade(&app));
        *app.self_weak.borrow_mut() = Rc::downgrade(&app);
        app.bind_callbacks();
        Ok(app)
    }

    fn start(&self) {
        self.render();
        self.load_saved_reviews();
        let weak = self.weak_self();
        *self.refresh_timer.borrow_mut() =
            Some(self.ui.tasks().every(REVIEW_REFRESH_INTERVAL, move || {
                if let Some(app) = weak.upgrade() {
                    app.refresh_open_reviews();
                }
            }));
        self.view.show();
    }

    fn weak_self(&self) -> Weak<Self> {
        self.self_weak.borrow().clone()
    }

    fn bind_callbacks(&self) {
        self.view.open_review.sheet.on_response({
            let weak = self.weak_self();
            move |response| {
                if response == DialogResponse::Primary
                    && let Some(app) = weak.upgrade()
                {
                    app.open_review_from_form();
                }
            }
        });
        self.view.open_review.url.on_change({
            let weak = self.weak_self();
            move |value| {
                if let Some(app) = weak.upgrade() {
                    let mut state = app.state.borrow_mut();
                    state.main.pull_request_url = value.to_string();
                    app.view
                        .open_review
                        .sheet
                        .set_button_enabled(DialogResponse::Primary, state.main.can_open_review());
                }
            }
        });
        self.view.open_review.repository.on_change({
            let weak = self.weak_self();
            move |value| {
                if let Some(app) = weak.upgrade() {
                    app.state.borrow_mut().main.local_repository_path = value.to_string();
                }
            }
        });
        self.view.open_review.url.on_submit({
            let weak = self.weak_self();
            move |_| {
                if let Some(app) = weak.upgrade()
                    && app.state.borrow().main.can_open_review()
                {
                    app.view.open_review.sheet.hide();
                    app.open_review_from_form();
                }
            }
        });
        self.view.open_review.inline_url.on_submit({
            let weak = self.weak_self();
            move |_| {
                if let Some(app) = weak.upgrade() {
                    app.open_review_from_inline_form();
                }
            }
        });
        self.view.open_review.inline_url.on_change({
            let weak = self.weak_self();
            move |value| {
                if let Some(app) = weak.upgrade() {
                    let mut state = app.state.borrow_mut();
                    state.main.pull_request_url = value.to_string();
                    app.view
                        .open_review
                        .inline_open
                        .set_enabled(state.main.can_open_review());
                }
            }
        });
        self.view.open_review.inline_repository.on_change({
            let weak = self.weak_self();
            move |value| {
                if let Some(app) = weak.upgrade() {
                    app.state.borrow_mut().main.local_repository_path = value.to_string();
                }
            }
        });
        self.view.open_review.inline_open.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.open_review_from_inline_form();
                }
            }
        });
        self.view.open_review.inline_cancel.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.state.borrow_mut().main.hide_open_review_panel();
                    app.render();
                }
            }
        });
        self.view.open_review.browse_button.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.view.open_review.browse.open();
                }
            }
        });
        self.view.open_review.inline_browse_button.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.view.open_review.inline_browse.open();
                }
            }
        });
        self.view.open_review.browse.on_select({
            let weak = self.weak_self();
            move |selection| {
                let Some(path) = selection.first().and_then(naui::FileEntry::path) else {
                    return;
                };
                if let Some(app) = weak.upgrade() {
                    app.view
                        .open_review
                        .repository
                        .set_text(&path.to_string_lossy());
                }
            }
        });
        self.view.open_review.inline_browse.on_select({
            let weak = self.weak_self();
            move |selection| {
                let Some(path) = selection.first().and_then(naui::FileEntry::path) else {
                    return;
                };
                if let Some(app) = weak.upgrade() {
                    app.view
                        .open_review
                        .inline_repository
                        .set_text(&path.to_string_lossy());
                }
            }
        });
        self.view.toolbar.on_activate({
            let weak = self.weak_self();
            move |index| {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                match index {
                    0 => app.show_open_review(),
                    1 => app.refresh_active_review(),
                    3 => app.navigate_change(-1),
                    4 => app.navigate_change(1),
                    5 => {
                        {
                            let mut state = app.state.borrow_mut();
                            state.main.toggle_code_view();
                            let enabled = state.main.is_code_view_expanded;
                            if let Some(review) = state.active_review_mut() {
                                review.set_focused_comment_mode(enabled);
                            }
                        }
                        app.render();
                    }
                    7 => app.mark_reviewed(),
                    9 => app.show_description(),
                    _ => {}
                }
            }
        });
        self.view.sidebar.on_select({
            let weak = self.weak_self();
            move |index| {
                if let Some(app) = weak.upgrade() {
                    app.select_sidebar_item(index);
                }
            }
        });
        self.view.status.authenticate.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.authenticate_and_retry();
                }
            }
        });
        self.view.status.error_bar.on_close({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.state.borrow_mut().main.error_message.clear();
                    app.render();
                }
            }
        });
        self.view.activity.back.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.state.borrow_mut().main.hide_activity();
                    app.render();
                }
            }
        });
        self.view.activity.error.on_close({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.state.borrow_mut().activity.error_message.clear();
                    app.render_activity();
                }
            }
        });
        self.view.activity.previous_month.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.state.borrow_mut().activity.previous_month();
                    app.render();
                    app.load_activity();
                }
            }
        });
        self.view.activity.next_month.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade()
                    && app.state.borrow_mut().activity.next_month().is_some()
                {
                    app.render();
                    app.load_activity();
                }
            }
        });
        self.view.description.sheet.on_response({
            let _weak = self.weak_self();
            move |_| {}
        });
        self.view.completion.sheet.on_response({
            let weak = self.weak_self();
            move |response| {
                if response == DialogResponse::Primary
                    && let Some(app) = weak.upgrade()
                {
                    app.complete_review();
                }
            }
        });
        self.view.deletion.sheet.on_response({
            let weak = self.weak_self();
            move |response| {
                if response == DialogResponse::Primary
                    && let Some(app) = weak.upgrade()
                {
                    app.delete_comment();
                }
            }
        });
        self.view.focused_comment.sheet.on_response({
            let weak = self.weak_self();
            move |response| {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                match response {
                    DialogResponse::Primary => app.save_focused_comment(),
                    DialogResponse::Cancel => {
                        if let Some(review) = app.state.borrow_mut().active_review_mut() {
                            review.cancel_comment_draft();
                        }
                        app.render();
                    }
                    DialogResponse::Secondary => {}
                }
            }
        });
        self.view.focused_comment.comment.on_change({
            let weak = self.weak_self();
            move |text| {
                if let Some(app) = weak.upgrade()
                    && let Some(review) = app.state.borrow_mut().active_review_mut()
                {
                    review.draft_comment = text.to_string();
                    app.view
                        .focused_comment
                        .sheet
                        .set_button_enabled(DialogResponse::Primary, review.can_save_comment());
                }
            }
        });
        self.view.focused_comment.suggestion.on_change({
            let weak = self.weak_self();
            move |text| {
                if let Some(app) = weak.upgrade()
                    && let Some(review) = app.state.borrow_mut().active_review_mut()
                {
                    review.draft_suggestion = text.to_string();
                    app.view
                        .focused_comment
                        .sheet
                        .set_button_enabled(DialogResponse::Primary, review.can_save_comment());
                }
            }
        });
        self.view.workspace.files.search.on_change({
            let weak = self.weak_self();
            move |value| {
                if let Some(app) = weak.upgrade() {
                    app.set_file_filter(value);
                }
            }
        });
        self.view.workspace.files.tree.on_select({
            let weak = self.weak_self();
            move |path| {
                if let Some(app) = weak.upgrade()
                    && !path.is_empty()
                {
                    app.select_file_tree_path(path);
                }
            }
        });
        self.view.workspace.files.tree.on_expand({
            let weak = self.weak_self();
            move |path, expanded| {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                if let Some(review) = app.state.borrow_mut().active_review_mut()
                    && let Some(node) = file_node_at_path_mut(&mut review.file_roots, path)
                {
                    node.is_expanded = expanded;
                }
            }
        });
        self.view.workspace.iteration.on_select({
            let weak = self.weak_self();
            move |index| {
                if let Some(app) = weak.upgrade() {
                    app.select_iteration(index);
                }
            }
        });
        self.view.workspace.files_splitter.on_resize({
            let weak = self.weak_self();
            move |delta| {
                if let Some(app) = weak.upgrade() {
                    app.state.borrow_mut().main.resize_changed_files_pane(delta);
                    app.render_focus_layout();
                }
            }
        });
        self.view.workspace.inspector_splitter.on_resize({
            let weak = self.weak_self();
            move |delta| {
                if let Some(app) = weak.upgrade() {
                    app.state
                        .borrow_mut()
                        .main
                        .resize_review_inspector_pane(delta);
                    app.render_focus_layout();
                }
            }
        });
        self.view.workspace.archive_menu.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.set_active_review_archived(true);
                }
            }
        });
        self.view.workspace.reopen_menu.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.set_active_review_archived(false);
                }
            }
        });
        self.view.workspace.complete_menu.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.request_complete();
                }
            }
        });
        self.view.workspace.diff.layout.on_select({
            let weak = self.weak_self();
            move |index| {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                if let Some(review) = app.state.borrow_mut().active_review_mut() {
                    review.set_diff_layout(if index == 1 {
                        DiffLayout::SideBySide
                    } else {
                        DiffLayout::Inline
                    });
                }
                app.render_diff();
            }
        });
        self.view.workspace.diff.context.on_select({
            let weak = self.weak_self();
            move |index| {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                let context = [3_u32, 5, 10, 20, 40].get(index).copied().unwrap_or(3);
                if let Some(review) = app.state.borrow_mut().active_review_mut() {
                    review.set_context_lines(context);
                }
                app.load_active_diff();
            }
        });
        self.view.workspace.diff.search.on_change({
            let weak = self.weak_self();
            move |query| {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                if let Some(review) = app.state.borrow_mut().active_review_mut() {
                    review.set_search_query(query);
                }
                app.render_diff();
            }
        });
        self.view.workspace.diff.search.on_submit({
            let weak = self.weak_self();
            move |_| {
                if let Some(app) = weak.upgrade() {
                    app.navigate_search(1);
                }
            }
        });
        self.view.workspace.diff.previous_search.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.navigate_search(-1);
                }
            }
        });
        self.view.workspace.diff.next_search.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.navigate_search(1);
                }
            }
        });
        self.view.workspace.diff.collapse.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    if let Some(review) = app.state.borrow_mut().active_review_mut() {
                        review.collapse_all_functions();
                    }
                    app.render_diff();
                }
            }
        });
        self.view.workspace.diff.expand.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    if let Some(review) = app.state.borrow_mut().active_review_mut() {
                        review.expand_all_functions();
                    }
                    app.render_diff();
                }
            }
        });
        self.view.workspace.diff.markdown_toggle.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    if let Some(review) = app.state.borrow_mut().active_review_mut() {
                        review.toggle_markdown_format();
                    }
                    app.render_diff();
                }
            }
        });
        self.view.workspace.diff.back_to_diff.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    if let Some(review) = app.state.borrow_mut().active_review_mut() {
                        review.return_to_diff();
                    }
                    app.render();
                }
            }
        });
        self.view.workspace.diff.table.on_select({
            let weak = self.weak_self();
            move |rows| {
                if let Some(app) = weak.upgrade()
                    && let Some(index) = rows.first()
                {
                    app.select_diff_row(*index);
                }
            }
        });
        self.view.workspace.diff.marker.on_pointer({
            let weak = self.weak_self();
            move |event| {
                if event.phase == naui::PointerPhase::Up
                    && event.button == Some(naui::PointerButton::Primary)
                    && let Some(app) = weak.upgrade()
                {
                    app.navigate_to_marker(event.point.y);
                }
            }
        });
        self.view.workspace.inspector.tabs.on_select({
            let weak = self.weak_self();
            move |index| {
                if let Some(app) = weak.upgrade()
                    && let Some(review) = app.state.borrow_mut().active_review_mut()
                {
                    review.inspector_tab = match index {
                        1 => crate::state::InspectorTab::Overview,
                        2 => crate::state::InspectorTab::Notes,
                        _ => crate::state::InspectorTab::Comments,
                    };
                }
            }
        });
        self.view.workspace.inspector.error.on_close({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    if let Some(review) = app.state.borrow_mut().active_review_mut() {
                        review.clear_error();
                    }
                    app.render();
                }
            }
        });
        self.view.workspace.inspector.overview_link.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade()
                    && let Some(url) = app
                        .state
                        .borrow()
                        .active_review()
                        .and_then(|review| review.session.as_ref())
                        .map(|session| session.details.reference.web_url.clone())
                        .filter(|url| !url.trim().is_empty())
                {
                    let _ = app.ui.open_url(&url);
                }
            }
        });
        self.view
            .workspace
            .inspector
            .comments
            .collapse_all
            .on_click({
                let weak = self.weak_self();
                move || {
                    if let Some(app) = weak.upgrade() {
                        if let Some(review) = app.state.borrow_mut().active_review_mut() {
                            review.collapse_all_comments();
                        }
                        app.render_comments();
                        app.render_diff();
                    }
                }
            });
        self.view.workspace.inspector.comments.expand_all.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    if let Some(review) = app.state.borrow_mut().active_review_mut() {
                        review.expand_all_comments();
                    }
                    app.render_comments();
                    app.render_diff();
                }
            }
        });
        self.view.workspace.inspector.notes.on_change({
            let weak = self.weak_self();
            move |notes| {
                if let Some(app) = weak.upgrade() {
                    app.save_notes_debounced(notes);
                }
            }
        });
        self.view.workspace.inspector.save_notes.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.save_notes_now();
                }
            }
        });
        self.view
            .workspace
            .inspector
            .comments
            .current_file
            .on_toggle({
                let weak = self.weak_self();
                move |checked| {
                    if let Some(app) = weak.upgrade() {
                        if let Some(review) = app.state.borrow_mut().active_review_mut() {
                            review.show_only_current_file_comments = checked;
                        }
                        app.render_comments();
                    }
                }
            });
        self.view.workspace.inspector.comments.hide_bots.on_toggle({
            let weak = self.weak_self();
            move |checked| {
                if let Some(app) = weak.upgrade() {
                    if let Some(review) = app.state.borrow_mut().active_review_mut() {
                        review.hide_bot_comments = checked;
                    }
                    app.render_comments();
                }
            }
        });
        self.view.workspace.inspector.comments.draft.on_change({
            let weak = self.weak_self();
            move |text| {
                if let Some(app) = weak.upgrade()
                    && let Some(review) = app.state.borrow_mut().active_review_mut()
                {
                    review.draft_comment = text.to_string();
                    app.view
                        .workspace
                        .inspector
                        .comments
                        .save_comment
                        .set_enabled(review.can_save_comment());
                }
            }
        });
        self.view
            .workspace
            .inspector
            .comments
            .suggestion
            .on_change({
                let weak = self.weak_self();
                move |text| {
                    if let Some(app) = weak.upgrade()
                        && let Some(review) = app.state.borrow_mut().active_review_mut()
                    {
                        review.draft_suggestion = text.to_string();
                        app.view
                            .workspace
                            .inspector
                            .comments
                            .save_comment
                            .set_enabled(review.can_save_comment());
                    }
                }
            });
        self.view
            .workspace
            .inspector
            .comments
            .save_comment
            .on_click({
                let weak = self.weak_self();
                move || {
                    if let Some(app) = weak.upgrade() {
                        app.save_comment();
                    }
                }
            });
        self.view.workspace.inspector.comments.publish.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.publish_comments();
                }
            }
        });

        self.view.window.on_close_request({
            let weak = self.weak_self();
            move || {
                weak.upgrade()
                    .map_or(CloseResponse::Close, |app| app.request_close())
            }
        });
        self.bind_shortcut(Shortcut::new('l'), || {});
        self.bind_shortcut(Shortcut::new('o'), || {});
        self.bind_shortcut(Shortcut::new('f'), || {});
    }

    fn bind_shortcut(&self, shortcut: Shortcut, fallback: impl FnMut() + 'static) {
        let mut fallback = fallback;
        let weak = self.weak_self();
        self.view.window.on_shortcut(shortcut, move || {
            if let Some(app) = weak.upgrade() {
                match shortcut {
                    value if value == Shortcut::new('l') || value == Shortcut::new('o') => {
                        app.show_open_review();
                    }
                    value if value == Shortcut::new('f') => {
                        app.view.workspace.diff.search.request_focus();
                    }
                    _ => fallback(),
                }
            }
        });
        if shortcut == Shortcut::new('o') {
            let weak = self.weak_self();
            self.view
                .window
                .on_shortcut(Shortcut::function(5), move || {
                    if let Some(app) = weak.upgrade() {
                        app.refresh_active_review();
                    }
                });
            let weak = self.weak_self();
            self.view
                .window
                .on_shortcut(Shortcut::function(7), move || {
                    if let Some(app) = weak.upgrade() {
                        app.navigate_change(1);
                    }
                });
            let weak = self.weak_self();
            self.view
                .window
                .on_shortcut(Shortcut::function(7).shift(true), move || {
                    if let Some(app) = weak.upgrade() {
                        app.navigate_change(-1);
                    }
                });
            let weak = self.weak_self();
            self.view
                .window
                .on_shortcut(Shortcut::function(3), move || {
                    if let Some(app) = weak.upgrade() {
                        app.navigate_search(1);
                    }
                });
            let weak = self.weak_self();
            self.view
                .window
                .on_shortcut(Shortcut::function(3).shift(true), move || {
                    if let Some(app) = weak.upgrade() {
                        app.navigate_search(-1);
                    }
                });
            let weak = self.weak_self();
            self.view
                .window
                .on_shortcut(Shortcut::function(12), move || {
                    if let Some(app) = weak.upgrade() {
                        app.navigate_definition();
                    }
                });
            let weak = self.weak_self();
            self.view
                .window
                .on_shortcut(Shortcut::function(11), move || {
                    if let Some(app) = weak.upgrade() {
                        {
                            let mut state = app.state.borrow_mut();
                            state.main.toggle_code_view();
                            let enabled = state.main.is_code_view_expanded;
                            if let Some(review) = state.active_review_mut() {
                                review.set_focused_comment_mode(enabled);
                            }
                        }
                        app.render();
                    }
                });
            let weak = self.weak_self();
            self.view
                .window
                .on_shortcut(Shortcut::from_key(Key::Escape), move || {
                    if let Some(app) = weak.upgrade()
                        && !app.any_dialog_open()
                        && app.state.borrow().main.is_code_view_expanded
                    {
                        app.state.borrow_mut().main.toggle_code_view();
                        if let Some(review) = app.state.borrow_mut().active_review_mut() {
                            review.set_focused_comment_mode(false);
                        }
                        app.render();
                    }
                });
            let weak = self.weak_self();
            self.view
                .window
                .on_shortcut(Shortcut::from_key(Key::Enter).primary(true), move || {
                    if let Some(app) = weak.upgrade() {
                        app.save_comment();
                    }
                });
            let weak = self.weak_self();
            self.view.window.on_shortcut(
                Shortcut::from_key(Key::Enter).primary(true).shift(true),
                move || {
                    if let Some(app) = weak.upgrade() {
                        app.publish_comments();
                    }
                },
            );
        }
    }

    fn spawn<F>(&self, future: F)
    where
        F: Future<Output = UiEvent> + Send + 'static,
    {
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let event = future.await;
            let _ = sender.send(event);
        });
    }

    fn load_saved_reviews(&self) {
        {
            let mut state = self.state.borrow_mut();
            state.main.is_opening = true;
            state.main.status_message = "Restoring local reviews...".into();
        }
        self.render();
        let services = self.services.clone();
        self.spawn(async move {
            let result = services.load_all(None).await.map_err(UiFailure::from);
            Box::new(move |app: &AppController| app.finish_load_saved_reviews(result)) as UiEvent
        });
    }

    fn finish_load_saved_reviews(
        &self,
        result: std::result::Result<Vec<ReviewSessionState>, UiFailure>,
    ) {
        match result {
            Ok(states) => {
                let mut controller = self.state.borrow_mut();
                controller.main = MainAppState::from_review_states(states.iter());
                controller.reviews.clear();
                for session in &states {
                    controller.reviews.insert(
                        session.details.reference.session_key.clone(),
                        ReviewSessionAppState::from_session_state(session),
                    );
                }
                controller.main.is_opening = false;
                controller.main.status_message = if states.is_empty() {
                    String::new()
                } else {
                    format!("Restored {} local review(s).", states.len())
                };
                let is_empty = states.is_empty();
                drop(controller);
                self.render();
                if is_empty {
                    self.view.open_review.inline_url.request_focus();
                    let weak = self.weak_self();
                    let _ = self.ui.tasks().after(Duration::from_millis(120), move || {
                        if let Some(app) = weak.upgrade()
                            && app.state.borrow().main.active_review_key.is_none()
                            && !app.state.borrow().main.is_activity_view_visible
                            && !app.any_dialog_open()
                        {
                            app.view.open_review.inline_url.request_focus();
                        }
                    });
                }
                self.load_active_diff();
                for session in states {
                    let key = session.details.reference.session_key.clone();
                    self.refresh_summary(&key, session);
                }
            }
            Err(error) => self.finish_error(error, None),
        }
    }

    fn refresh_summary(&self, session_key: &str, mut session: ReviewSessionState) {
        let services = self.services.clone();
        let key = session_key.to_string();
        self.spawn(async move {
            let result = services
                .refresh_summary(&mut session, None)
                .await
                .map(|()| session)
                .map_err(UiFailure::from);
            Box::new(move |app: &AppController| app.finish_refresh_summary(&key, result)) as UiEvent
        });
    }

    fn finish_refresh_summary(
        &self,
        session_key: &str,
        result: std::result::Result<ReviewSessionState, UiFailure>,
    ) {
        match result {
            Ok(session) => {
                let mut state = self.state.borrow_mut();
                if let Some(review) = state.reviews.get_mut(session_key) {
                    review.replace_summary_state_preserving_view(&session);
                }
                let sessions = state
                    .reviews
                    .values()
                    .filter_map(|review| review.session.clone())
                    .collect::<Vec<_>>();
                state.main.set_reviews(&sessions);
                drop(state);
                self.render();
            }
            Err(error) => self.finish_error(
                error,
                Some(PendingAction::RefreshReview {
                    session_key: session_key.to_string(),
                }),
            ),
        }
    }

    fn open_review_from_form(&self) {
        let url = self.view.open_review.url.text().trim().to_string();
        let repository = self.view.open_review.repository.text().trim().to_string();
        self.open_review(
            url,
            (!repository.is_empty()).then(|| PathBuf::from(repository)),
        );
    }

    fn open_review_from_inline_form(&self) {
        let url = self.view.open_review.inline_url.text().trim().to_string();
        let repository = self
            .view
            .open_review
            .inline_repository
            .text()
            .trim()
            .to_string();
        self.open_review(
            url,
            (!repository.is_empty()).then(|| PathBuf::from(repository)),
        );
    }

    fn show_open_review(&self) {
        let open_inline = {
            let mut state = self.state.borrow_mut();
            if state.main.is_code_view_expanded {
                state.main.is_code_view_expanded = false;
                if let Some(review) = state.active_review_mut() {
                    review.set_focused_comment_mode(false);
                }
            }
            if state.main.active_review_key.is_none() {
                state.main.show_open_review_panel();
                true
            } else {
                false
            }
        };
        self.render();
        if open_inline {
            self.view.open_review.inline_url.request_focus();
        } else {
            let (url, repo, can_open) = {
                let state = self.state.borrow();
                (
                    state.main.pull_request_url.clone(),
                    state.main.local_repository_path.clone(),
                    state.main.can_open_review(),
                )
            };
            self.view.open_review.url.set_text(&url);
            self.view.open_review.repository.set_text(&repo);
            self.view
                .open_review
                .sheet
                .set_button_enabled(DialogResponse::Primary, can_open);
            self.view.open_review.sheet.open();
        }
    }

    fn open_review(&self, url: String, repository_path: Option<PathBuf>) {
        if url.trim().is_empty() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if state.main.is_opening {
                return;
            }
            state.main.is_opening = true;
            state.main.error_message.clear();
            state.main.status_message = "Opening pull request...".into();
            state.main.pull_request_url.clone_from(&url);
            state.main.local_repository_path = repository_path
                .as_deref()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        self.render();
        let services = self.services.clone();
        let retry = PendingAction::OpenReview {
            url: url.clone(),
            repository_path: repository_path.clone(),
        };
        self.spawn(async move {
            let result = services
                .open(&url, repository_path.as_deref(), None)
                .await
                .map_err(UiFailure::from);
            Box::new(move |app: &AppController| app.finish_open_review(result, retry)) as UiEvent
        });
    }

    fn finish_open_review(
        &self,
        result: std::result::Result<ReviewSessionState, UiFailure>,
        retry: PendingAction,
    ) {
        match result {
            Ok(session) => {
                let key = session.details.reference.session_key.clone();
                {
                    let mut state = self.state.borrow_mut();
                    state.reviews.insert(
                        key.clone(),
                        ReviewSessionAppState::from_session_state(&session),
                    );
                    state.main.track_review(&session);
                    let _ = state.main.select_review(&key);
                    state.main.is_opening = false;
                    state.main.pull_request_url.clear();
                    state.main.local_repository_path.clear();
                    state.main.status_message = format!(
                        "Opened #{} · {}.",
                        session.details.reference.number, session.details.title
                    );
                    state.main.clear_authentication_required();
                    state.authentication_reference = None;
                    state.pending_action = None;
                }
                self.view.open_review.url.set_text("");
                self.view.open_review.repository.set_text("");
                self.view.open_review.inline_url.set_text("");
                self.view.open_review.inline_repository.set_text("");
                self.render();
                self.load_active_diff();
            }
            Err(error) => self.finish_error(error, Some(retry)),
        }
    }

    fn select_sidebar_item(&self, index: usize) {
        let selection = {
            let mut state = self.state.borrow_mut();
            let active_count = state.main.active_reviews.len();
            let reviewed_count = state.main.reviewed_reviews.len();
            let session_key = if index < active_count {
                Some(state.main.active_reviews[index].session_key.clone())
            } else if index < active_count + reviewed_count {
                Some(
                    state.main.reviewed_reviews[index - active_count]
                        .session_key
                        .clone(),
                )
            } else if index == active_count + reviewed_count {
                None
            } else {
                return;
            };
            if let Some(session_key) = session_key {
                if state.main.active_review_key.as_deref() == Some(session_key.as_str())
                    && !state.main.is_activity_view_visible
                    && !state.main.is_open_review_panel_visible
                {
                    return;
                }
                state.main.select_review(&session_key);
                Some(session_key)
            } else {
                if state.main.is_activity_view_visible {
                    return;
                }
                state.main.show_activity();
                None
            }
        };
        if selection.is_some() {
            self.render();
            self.load_active_diff();
        } else {
            self.render();
            self.load_activity();
        }
    }

    fn refresh_active_review(&self) {
        let Some((key, session)) = self.active_session_snapshot() else {
            return;
        };
        self.refresh_review(&key, session);
    }

    fn refresh_review(&self, session_key: &str, mut session: ReviewSessionState) {
        {
            let mut state = self.state.borrow_mut();
            if let Some(review) = state.reviews.get_mut(session_key) {
                review.set_busy(true);
            }
        }
        self.render();
        let services = self.services.clone();
        let key = session_key.to_string();
        self.spawn(async move {
            let result = services
                .refresh_repository(&mut session, None)
                .await
                .map(|()| session)
                .map_err(UiFailure::from);
            Box::new(move |app: &AppController| {
                app.finish_refresh_review(
                    &key,
                    result,
                    PendingAction::RefreshReview {
                        session_key: key.clone(),
                    },
                )
            }) as UiEvent
        });
    }

    fn finish_refresh_review(
        &self,
        session_key: &str,
        result: std::result::Result<ReviewSessionState, UiFailure>,
        retry: PendingAction,
    ) {
        match result {
            Ok(session) => {
                {
                    let mut state = self.state.borrow_mut();
                    if let Some(review) = state.reviews.get_mut(session_key) {
                        review.replace_session_state_preserving_local_edits(&session);
                        review.set_busy(false);
                        review.set_status("Pull request refreshed.");
                    }
                    let sessions = state
                        .reviews
                        .values()
                        .filter_map(|review| review.session.clone())
                        .collect::<Vec<_>>();
                    state.main.set_reviews(&sessions);
                }
                self.render();
                if self
                    .state
                    .borrow()
                    .main
                    .active_review_key
                    .as_deref()
                    .is_some_and(|key| key == session_key)
                {
                    self.load_active_diff();
                }
            }
            Err(error) => self.finish_error(error, Some(retry)),
        }
    }

    fn refresh_open_reviews(&self) {
        let snapshots = {
            let state = self.state.borrow();
            state
                .main
                .active_reviews
                .iter()
                .filter_map(|item| {
                    state
                        .reviews
                        .get(&item.session_key)
                        .and_then(ReviewSessionAppState::persisted_snapshot)
                        .map(|session| (item.session_key.clone(), session))
                })
                .collect::<Vec<_>>()
        };
        for (key, session) in snapshots {
            self.refresh_review(&key, session);
        }
        self.refresh_generation
            .set(self.refresh_generation.get().wrapping_add(1));
    }

    fn select_iteration(&self, index: usize) {
        let selection = {
            let mut state = self.state.borrow_mut();
            let Some(review) = state.active_review_mut() else {
                return;
            };
            let iteration_id = review
                .iterations
                .get(index)
                .and_then(|iteration| (!iteration.is_all_changes).then(|| iteration.id.clone()));
            if !review.select_iteration(iteration_id.as_deref()) {
                return;
            }
            let Some(session_key) = review
                .session
                .as_ref()
                .map(|session| session.details.reference.session_key.clone())
            else {
                return;
            };
            let session = review.persisted_snapshot();
            (session_key, iteration_id, session)
        };
        self.render();
        let Some(session) = selection.2 else {
            return;
        };
        let mut session = session;
        let services = self.services.clone();
        let retry = PendingAction::SelectIteration {
            session_key: selection.0.clone(),
            iteration_id: selection.1.clone(),
        };
        self.spawn(async move {
            let result = services
                .select_iteration(&mut session, selection.1.as_deref(), None)
                .await
                .map(|()| session)
                .map_err(UiFailure::from);
            Box::new(move |app: &AppController| {
                app.finish_select_iteration(&selection.0, result, retry)
            }) as UiEvent
        });
    }

    fn finish_select_iteration(
        &self,
        session_key: &str,
        result: std::result::Result<ReviewSessionState, UiFailure>,
        retry: PendingAction,
    ) {
        match result {
            Ok(session) => {
                if let Some(review) = self.state.borrow_mut().reviews.get_mut(session_key) {
                    review.replace_session_state(&session);
                }
                self.render();
                self.load_active_diff();
            }
            Err(error) => self.finish_error(error, Some(retry)),
        }
    }

    fn select_file_tree_path(&self, path: &[usize]) {
        let selected = {
            let mut state = self.state.borrow_mut();
            state.pending_comment_jump = None;
            let Some(review) = state.active_review_mut() else {
                return;
            };
            let Some(node_path) =
                file_node_at_path(&review.file_roots, path).map(|node| node.path.clone())
            else {
                return;
            };
            if !review.select_file_or_folder(&node_path) {
                return;
            }
            (
                node_path,
                review.context_lines,
                !review.selected_path_is_folder,
                review.persisted_snapshot(),
            )
        };
        self.render();
        if let Some(snapshot) = selected.3 {
            self.persist_snapshot(
                snapshot,
                Some("Selection saved on this device.".into()),
                None,
            );
        }
        self.load_diff_for_path(selected.0.clone(), selected.1 as i32, selected.2);
    }

    fn set_file_filter(&self, value: &str) {
        if let Some(review) = self.state.borrow_mut().active_review_mut() {
            review.set_file_filter(value);
        }
        self.render_file_tree();
    }

    fn load_active_diff(&self) {
        let request = {
            let state = self.state.borrow();
            let Some(review) = state.active_review() else {
                return;
            };
            if review.selected_file_path.trim().is_empty() {
                return;
            }
            Some((
                review.selected_file_path.clone(),
                review.context_lines,
                !review.selected_path_is_folder,
            ))
        };
        if let Some((path, context, complete_file)) = request {
            self.load_diff_for_path(path, context as i32, complete_file);
        }
    }

    fn load_diff_for_path(&self, path: String, context_lines: i32, complete_file: bool) {
        let request = {
            let mut state = self.state.borrow_mut();
            let Some(review) = state.active_review_mut() else {
                return;
            };
            review.set_busy(true);
            let Some(session) = review.persisted_snapshot() else {
                return;
            };
            (session.details.reference.session_key.clone(), session)
        };
        self.render();
        let services = self.services.clone();
        let retry = PendingAction::LoadDiff {
            session_key: request.0.clone(),
            path: path.clone(),
            context_lines,
            complete_file,
        };
        self.spawn(async move {
            let result = services
                .load_diff(&request.1, &path, context_lines, complete_file, None)
                .await
                .map_err(UiFailure::from);
            let markdown = if path
                .rsplit_once('.')
                .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("md"))
            {
                services.read_file(&request.1, &path, None, None).await.ok()
            } else {
                None
            };
            Box::new(move |app: &AppController| {
                app.finish_load_diff(request.0, path, result, markdown, retry)
            }) as UiEvent
        });
    }

    fn finish_load_diff(
        &self,
        session_key: String,
        path: String,
        result: std::result::Result<Vec<DiffBlock>, UiFailure>,
        markdown: Option<String>,
        retry: PendingAction,
    ) {
        let loaded_complete_file = matches!(
            &retry,
            PendingAction::LoadDiff {
                complete_file: true,
                ..
            }
        );
        match result {
            Ok(blocks) => {
                let mut state = self.state.borrow_mut();
                let pending_comment_jump = state.pending_comment_jump.clone();
                let mut clear_pending_jump = false;
                let mut retry_complete_file = false;
                let Some(review) = state.reviews.get_mut(&session_key) else {
                    return;
                };
                if !review
                    .selected_file_path
                    .replace('\\', "/")
                    .eq_ignore_ascii_case(&path.replace('\\', "/"))
                {
                    review.set_busy(false);
                    drop(state);
                    return;
                }
                review.set_busy(false);
                review.replace_diff_blocks(&blocks);
                if let Some(markdown) = markdown {
                    let _ = review.set_markdown_content(&markdown);
                }
                review.set_status(format!("Loaded {}.", review.selected_file_path));
                if let Some(jump) = pending_comment_jump.filter(|jump| {
                    jump.session_key.eq_ignore_ascii_case(&session_key)
                        && jump
                            .path
                            .replace('\\', "/")
                            .eq_ignore_ascii_case(&path.replace('\\', "/"))
                }) {
                    let source_index = review.diff.lines.iter().find(|line| {
                        line.line
                            .file_path
                            .replace('\\', "/")
                            .eq_ignore_ascii_case(&jump.path.replace('\\', "/"))
                            && match jump.side {
                                Some(ReviewCommentSide::Left) => {
                                    line.line.old_line_number == jump.line_number
                                }
                                Some(ReviewCommentSide::Right) => {
                                    line.line.new_line_number == jump.line_number
                                }
                                None => false,
                            }
                    });
                    if let Some(line) = source_index {
                        review.select_diff_line(line.source_index);
                        clear_pending_jump = true;
                    } else if loaded_complete_file {
                        clear_pending_jump = true;
                        review.set_status("The comment line is no longer present in this range.");
                    } else {
                        retry_complete_file = true;
                    }
                }
                if clear_pending_jump {
                    state.pending_comment_jump = None;
                }
                drop(state);
                self.render();
                if retry_complete_file {
                    self.load_diff_for_path(path, 3, true);
                }
            }
            Err(error) => self.finish_error(error, Some(retry)),
        }
    }

    fn persist_active(&self, success_message: Option<String>) {
        let snapshot = {
            let state = self.state.borrow();
            state
                .active_review()
                .and_then(ReviewSessionAppState::persisted_snapshot)
        };
        if let Some(snapshot) = snapshot {
            self.persist_snapshot(snapshot, success_message, None);
        }
    }

    fn persist_snapshot(
        &self,
        snapshot: ReviewSessionState,
        success_message: Option<String>,
        session_key: Option<String>,
    ) {
        let key = session_key.unwrap_or_else(|| snapshot.details.reference.session_key.clone());
        let services = self.services.clone();
        self.spawn(async move {
            let result = services
                .save(&snapshot, None)
                .await
                .map_err(UiFailure::from);
            Box::new(move |app: &AppController| app.finish_save(key, result, success_message))
                as UiEvent
        });
    }

    fn finish_save(
        &self,
        session_key: String,
        result: std::result::Result<(), UiFailure>,
        success_message: Option<String>,
    ) {
        let saved = result.is_ok();
        match result {
            Ok(()) => {
                let mut state = self.state.borrow_mut();
                if let Some(review) = state.reviews.get_mut(&session_key) {
                    review.mark_notes_saved();
                    if let Some(message) = success_message {
                        review.set_status(message);
                    }
                }
            }
            Err(error) => {
                self.finish_error(error, None);
                if let Some(review) = self.state.borrow_mut().reviews.get_mut(&session_key) {
                    review.mark_notes_save_failed("Save failed.");
                }
            }
        }
        self.render();
        let should_close = {
            let mut state = self.state.borrow_mut();
            if !state.close_after_save {
                false
            } else if !saved {
                state.close_after_save = false;
                state.pending_close_saves = 0;
                false
            } else {
                state.pending_close_saves = state.pending_close_saves.saturating_sub(1);
                let complete = state.pending_close_saves == 0;
                if complete {
                    state.close_after_save = false;
                }
                complete
            }
        };
        if should_close {
            self.view.window.close();
        }
    }

    fn request_close(&self) -> CloseResponse {
        let snapshots = self
            .state
            .borrow()
            .reviews
            .values()
            .filter(|review| review.notes_save_state == NotesSaveState::Saving)
            .filter_map(ReviewSessionAppState::persisted_snapshot)
            .collect::<Vec<_>>();
        if snapshots.is_empty() {
            return CloseResponse::Close;
        }
        {
            let mut state = self.state.borrow_mut();
            state.close_after_save = true;
            state.pending_close_saves = snapshots.len();
        }
        for (_, timer) in self.notes_timers.borrow_mut().drain() {
            timer.cancel();
        }
        for snapshot in snapshots {
            let session_key = snapshot.details.reference.session_key.clone();
            self.persist_snapshot(
                snapshot,
                Some("Notes saved before closing.".into()),
                Some(session_key),
            );
        }
        CloseResponse::KeepOpen
    }

    fn save_notes_debounced(&self, notes: &str) {
        let session_key = {
            let mut state = self.state.borrow_mut();
            let Some(review) = state.active_review_mut() else {
                return;
            };
            review.set_notes(notes);
            review
                .session
                .as_ref()
                .map(|session| session.details.reference.session_key.clone())
        };
        let Some(session_key) = session_key else {
            return;
        };
        if let Some(timer) = self.notes_timers.borrow_mut().remove(&session_key) {
            timer.cancel();
        }
        let weak = self.weak_self();
        let timer_session_key = session_key.clone();
        let timer = self.ui.tasks().after(NOTES_SAVE_DELAY, move || {
            if let Some(app) = weak.upgrade() {
                app.notes_timers.borrow_mut().remove(&timer_session_key);
                let snapshot = app
                    .state
                    .borrow()
                    .reviews
                    .get(&timer_session_key)
                    .and_then(ReviewSessionAppState::persisted_snapshot);
                if let Some(snapshot) = snapshot {
                    app.persist_snapshot(
                        snapshot,
                        Some("Notes saved on this device.".into()),
                        Some(timer_session_key),
                    );
                }
            }
        });
        self.notes_timers.borrow_mut().insert(session_key, timer);
        self.render_notes();
    }

    fn save_notes_now(&self) {
        let snapshot = {
            let state = self.state.borrow();
            state
                .active_review()
                .and_then(ReviewSessionAppState::persisted_snapshot)
        };
        let Some(snapshot) = snapshot else {
            return;
        };
        let session_key = snapshot.details.reference.session_key.clone();
        if let Some(timer) = self.notes_timers.borrow_mut().remove(&session_key) {
            timer.cancel();
        }
        self.persist_snapshot(
            snapshot,
            Some("Notes saved on this device.".into()),
            Some(session_key),
        );
    }

    fn save_comment(&self) {
        let result = {
            let mut state = self.state.borrow_mut();
            state
                .active_review_mut()
                .and_then(ReviewSessionAppState::queue_comment)
        };
        if result.is_some() {
            self.persist_active(Some("Comment queued locally.".into()));
            self.render();
        }
    }

    fn commit_comment_edit(&self, comment_id: String) {
        let request = {
            let mut state = self.state.borrow_mut();
            let Some(review) = state.active_review_mut() else {
                return;
            };
            let original = review
                .comments
                .iter()
                .find(|comment| comment.id().eq_ignore_ascii_case(&comment_id))
                .map(|comment| comment.comment.clone());
            if !review.commit_comment_edit(&comment_id) {
                return;
            }
            let Some(snapshot) = review.persisted_snapshot() else {
                return;
            };
            let key = snapshot.details.reference.session_key.clone();
            (key, snapshot, original)
        };
        let services = self.services.clone();
        self.spawn(async move {
            let result = services
                .save(&request.1, None)
                .await
                .map_err(UiFailure::from);
            Box::new(move |app: &AppController| {
                app.finish_comment_edit(request.0, comment_id, request.2, result)
            }) as UiEvent
        });
        self.render();
    }

    fn finish_comment_edit(
        &self,
        session_key: String,
        comment_id: String,
        original: Option<prs_core::ReviewComment>,
        result: std::result::Result<(), UiFailure>,
    ) {
        match result {
            Ok(()) => {
                if let Some(review) = self.state.borrow_mut().reviews.get_mut(&session_key) {
                    review.set_status("Saved comment changes locally.");
                }
            }
            Err(error) => {
                if let (Some(original), Some(review)) = (
                    original,
                    self.state.borrow_mut().reviews.get_mut(&session_key),
                ) {
                    if let Some(comment) = review
                        .comments
                        .iter_mut()
                        .find(|comment| comment.id().eq_ignore_ascii_case(&comment_id))
                    {
                        comment.comment = original.clone();
                    }
                    if let Some(session) = &mut review.session
                        && let Some(saved) = session
                            .comments
                            .iter_mut()
                            .find(|comment| comment.id.eq_ignore_ascii_case(&comment_id))
                    {
                        *saved = original;
                    }
                }
                self.finish_error(error, None);
            }
        }
        self.render();
    }

    fn publish_comments(&self) {
        let Some((key, mut session)) = self.active_session_snapshot() else {
            return;
        };
        {
            if let Some(review) = self.state.borrow_mut().reviews.get_mut(&key) {
                review.set_busy(true);
            }
        }
        self.render();
        let services = self.services.clone();
        let retry = PendingAction::PublishComments {
            session_key: key.clone(),
        };
        self.spawn(async move {
            let result = services
                .publish_pending_comments(&mut session, None)
                .await
                .map_err(UiFailure::from);
            Box::new(move |app: &AppController| {
                app.finish_publish_comments(key, session, result, retry)
            }) as UiEvent
        });
    }

    fn finish_publish_comments(
        &self,
        session_key: String,
        session: ReviewSessionState,
        result: std::result::Result<CommentPublishResult, UiFailure>,
        retry: PendingAction,
    ) {
        {
            let mut state = self.state.borrow_mut();
            if let Some(review) = state.reviews.get_mut(&session_key) {
                review.replace_session_state_preserving_view(&session);
                review.set_busy(false);
            }
        }
        match result {
            Ok(result) => {
                if let Some(review) = self.state.borrow_mut().reviews.get_mut(&session_key) {
                    review.set_status(if result.failed == 0 {
                        format!("Published {} comment(s).", result.published)
                    } else {
                        format!(
                            "Published {} comment(s); {} failed. Review the errors below.",
                            result.published, result.failed
                        )
                    });
                }
                self.render();
            }
            Err(error) => self.finish_error(error, Some(retry)),
        }
    }

    fn mark_reviewed(&self) {
        let Some((key, mut session)) = self.active_session_snapshot() else {
            return;
        };
        let services = self.services.clone();
        self.spawn(async move {
            let result = services
                .mark_iteration_reviewed(&mut session, None)
                .await
                .map(|()| session)
                .map_err(UiFailure::from);
            Box::new(move |app: &AppController| app.finish_mark_reviewed(key, result)) as UiEvent
        });
    }

    fn finish_mark_reviewed(
        &self,
        session_key: String,
        result: std::result::Result<ReviewSessionState, UiFailure>,
    ) {
        match result {
            Ok(session) => {
                if let Some(review) = self.state.borrow_mut().reviews.get_mut(&session_key) {
                    review.replace_session_state_preserving_view(&session);
                    review.set_status("Latest pull request iteration marked reviewed.");
                }
                self.render();
            }
            Err(error) => self.finish_error(error, None),
        }
    }

    fn any_dialog_open(&self) -> bool {
        self.view.open_review.sheet.is_open()
            || self.view.description.sheet.is_open()
            || self.view.completion.sheet.is_open()
            || self.view.deletion.sheet.is_open()
            || self.view.focused_comment.sheet.is_open()
    }

    fn show_focused_comment(&self) {
        let values = {
            let mut state = self.state.borrow_mut();
            let Some(review) = state.active_review_mut() else {
                return;
            };
            if !review.can_open_comment() {
                return;
            }
            review.prepare_focused_comment();
            let line_header = format_selected_line_description(review);
            let code_context = review
                .selected_diff_line()
                .map(|line| line.code_text().to_string())
                .unwrap_or_default();
            Some((
                line_header,
                code_context,
                review.draft_comment.clone(),
                review.draft_suggestion.clone(),
            ))
        };
        let Some((line_header, context, comment, suggestion)) = values else {
            return;
        };
        self.view.focused_comment.line_header.set_text(&line_header);
        self.view.focused_comment.context.set_text(&context);
        self.view
            .focused_comment
            .context
            .set_visible(!context.trim().is_empty());
        self.view.focused_comment.comment.set_text(&comment);
        self.view.focused_comment.suggestion.set_text(&suggestion);
        self.view.focused_comment.sheet.set_button_enabled(
            DialogResponse::Primary,
            self.state
                .borrow()
                .active_review()
                .is_some_and(ReviewSessionAppState::can_save_comment),
        );
        self.view.focused_comment.sheet.open();
    }

    fn save_focused_comment(&self) {
        {
            let mut state = self.state.borrow_mut();
            if let Some(review) = state.active_review_mut() {
                review
                    .draft_comment
                    .clone_from(&self.view.focused_comment.comment.text());
                review
                    .draft_suggestion
                    .clone_from(&self.view.focused_comment.suggestion.text());
                let _ = review.queue_comment();
            }
        }
        self.persist_active(Some("Comment saved locally.".into()));
        self.render();
    }

    fn request_complete(&self) {
        if self.state.borrow().active_review().is_some() {
            self.view.completion.sheet.open();
        }
    }

    fn set_active_review_archived(&self, archived: bool) {
        let Some((key, mut session)) = self.active_session_snapshot() else {
            return;
        };
        if archived == session.is_archived {
            return;
        }
        session.is_archived = archived;
        let services = self.services.clone();
        self.spawn(async move {
            let result = services
                .save(&session, None)
                .await
                .map(|()| session)
                .map_err(UiFailure::from);
            Box::new(move |app: &AppController| {
                app.finish_set_review_archived(key, archived, result)
            }) as UiEvent
        });
    }

    fn finish_set_review_archived(
        &self,
        session_key: String,
        archived: bool,
        result: std::result::Result<ReviewSessionState, UiFailure>,
    ) {
        match result {
            Ok(session) => {
                {
                    let mut state = self.state.borrow_mut();
                    if let Some(review) = state.reviews.get_mut(&session_key) {
                        review.replace_session_state_preserving_view(&session);
                    }
                    state.main.track_review(&session);
                    if archived {
                        state.main.close_selected_review();
                        state.main.status_message =
                            "Moved review to reviewed pull requests.".into();
                    } else {
                        state.main.select_review(&session_key);
                        state.main.status_message = "Review reopened.".into();
                    }
                }
                self.render();
                self.load_active_diff();
            }
            Err(error) => self.finish_error(error, None),
        }
    }

    fn complete_review(&self) {
        let Some((key, mut session)) = self.active_session_snapshot() else {
            return;
        };
        let services = self.services.clone();
        self.spawn(async move {
            let result = services
                .complete_review(&mut session, None)
                .await
                .map_err(UiFailure::from);
            Box::new(move |app: &AppController| app.finish_complete_review(key, result)) as UiEvent
        });
    }

    fn finish_complete_review(
        &self,
        session_key: String,
        result: std::result::Result<(), UiFailure>,
    ) {
        match result {
            Ok(()) => {
                let mut state = self.state.borrow_mut();
                state.reviews.remove(&session_key);
                state.main.remove_review(&session_key);
                state.main.status_message =
                    "Review completed. Local review data was cleaned up.".into();
                drop(state);
                self.render();
            }
            Err(error) => self.finish_error(error, None),
        }
    }

    fn request_delete_comment(&self, comment_id: String) {
        self.state.borrow_mut().pending_delete_comment = Some(comment_id);
        self.view.deletion.sheet.open();
    }

    fn delete_comment(&self) {
        let (key, comment_id, snapshot) = {
            let mut state = self.state.borrow_mut();
            let Some(comment_id) = state.pending_delete_comment.take() else {
                return;
            };
            let Some(review) = state.active_review_mut() else {
                return;
            };
            let Some(session) = review.persisted_snapshot() else {
                return;
            };
            (
                session.details.reference.session_key.clone(),
                comment_id,
                session,
            )
        };
        self.perform_delete_comment(key, comment_id, snapshot);
    }

    fn perform_delete_comment(
        &self,
        session_key: String,
        comment_id: String,
        mut session: ReviewSessionState,
    ) {
        let services = self.services.clone();
        let retry = PendingAction::DeleteComment {
            session_key: session_key.clone(),
            comment_id: comment_id.clone(),
        };
        self.spawn(async move {
            let result = services
                .delete_comment(&mut session, &comment_id, None)
                .await
                .map_err(UiFailure::from);
            Box::new(move |app: &AppController| {
                app.finish_delete_comment(session_key, session, result, retry)
            }) as UiEvent
        });
    }

    fn finish_delete_comment(
        &self,
        session_key: String,
        session: ReviewSessionState,
        result: std::result::Result<(), UiFailure>,
        retry: PendingAction,
    ) {
        match result {
            Ok(()) => {
                if let Some(review) = self.state.borrow_mut().reviews.get_mut(&session_key) {
                    review.replace_session_state_preserving_view(&session);
                    review.set_status("Comment deleted.".to_string());
                }
                self.render();
            }
            Err(error) => self.finish_error(error, Some(retry)),
        }
    }

    fn load_activity(&self) {
        let month = {
            let mut state = self.state.borrow_mut();
            state.activity.is_busy = true;
            state.activity.visible_month
        };
        self.render_activity();
        let activity = self.services.activity().clone();
        self.spawn(async move {
            let result = async {
                let days = activity
                    .get_month(month.year, month.month)
                    .await
                    .map_err(|error| UiFailure {
                        message: error.to_string(),
                        authentication_reference: None,
                    })?;
                let recent = activity
                    .get_recent_reviews(50)
                    .await
                    .map_err(|error| UiFailure {
                        message: error.to_string(),
                        authentication_reference: None,
                    })?;
                Ok::<_, UiFailure>((days, recent))
            }
            .await;
            Box::new(move |app: &AppController| app.finish_load_activity(month, result)) as UiEvent
        });
    }

    fn finish_load_activity(
        &self,
        month: ActivityMonth,
        result: std::result::Result<(Vec<ReviewActivityDay>, Vec<ReviewActivityRecord>), UiFailure>,
    ) {
        match result {
            Ok((days, recent)) => {
                self.state
                    .borrow_mut()
                    .activity
                    .load_month(month, &days, &recent);
            }
            Err(error) => {
                let mut state = self.state.borrow_mut();
                state.activity.is_busy = false;
                state.activity.error_message = error.message.clone();
            }
        }
        self.render();
    }

    fn active_session_snapshot(&self) -> Option<(String, ReviewSessionState)> {
        let state = self.state.borrow();
        let review = state.active_review()?;
        let session = review.persisted_snapshot()?;
        Some((session.details.reference.session_key.clone(), session))
    }

    fn authenticate_and_retry(&self) {
        let (reference, action) = {
            let state = self.state.borrow();
            (
                state.authentication_reference.clone(),
                state.pending_action.clone(),
            )
        };
        let Some(reference) = reference else {
            return;
        };
        let Some(action) = action else {
            return;
        };
        {
            let mut state = self.state.borrow_mut();
            state.main.is_opening = true;
            state.main.status_message =
                format!("Opening {} sign-in...", provider_label(&reference));
        }
        self.render();
        let services = self.services.clone();
        self.spawn(async move {
            let result = services
                .authenticate(&reference, None)
                .await
                .map_err(UiFailure::from);
            Box::new(move |app: &AppController| app.finish_authenticate(result, action)) as UiEvent
        });
    }

    fn finish_authenticate(
        &self,
        result: std::result::Result<(), UiFailure>,
        action: PendingAction,
    ) {
        match result {
            Ok(()) => {
                {
                    let mut state = self.state.borrow_mut();
                    state.main.is_opening = false;
                    state.main.clear_authentication_required();
                    state.authentication_reference = None;
                    state.pending_action = None;
                }
                self.retry_action(action);
            }
            Err(error) => self.finish_error(error, None),
        }
    }

    fn retry_action(&self, action: PendingAction) {
        match action {
            PendingAction::OpenReview {
                url,
                repository_path,
            } => self.open_review(url, repository_path),
            PendingAction::RefreshReview { session_key } => {
                if let Some(session) = self
                    .state
                    .borrow()
                    .reviews
                    .get(&session_key)
                    .and_then(ReviewSessionAppState::persisted_snapshot)
                {
                    self.refresh_review(&session_key, session);
                }
            }
            PendingAction::SelectIteration {
                session_key,
                iteration_id,
            } => {
                if let Some(mut session) = self
                    .state
                    .borrow()
                    .reviews
                    .get(&session_key)
                    .and_then(ReviewSessionAppState::persisted_snapshot)
                {
                    let retry = PendingAction::SelectIteration {
                        session_key: session_key.clone(),
                        iteration_id: iteration_id.clone(),
                    };
                    let services = self.services.clone();
                    self.spawn(async move {
                        let result = services
                            .select_iteration(&mut session, iteration_id.as_deref(), None)
                            .await
                            .map(|()| session)
                            .map_err(UiFailure::from);
                        Box::new(move |app: &AppController| {
                            app.finish_select_iteration(&session_key, result, retry)
                        }) as UiEvent
                    });
                }
            }
            PendingAction::LoadDiff {
                session_key,
                path,
                context_lines,
                complete_file,
            } => {
                self.state.borrow_mut().main.select_review(&session_key);
                self.load_diff_for_path(path, context_lines, complete_file);
            }
            PendingAction::PublishComments { session_key } => {
                self.state.borrow_mut().main.select_review(&session_key);
                self.publish_comments();
            }
            PendingAction::ReplyToComment {
                session_key,
                comment,
                body,
            } => self.reply_to_comment(session_key, *comment, body),
            PendingAction::DeleteComment {
                session_key,
                comment_id,
            } => {
                if let Some(session) = self
                    .state
                    .borrow()
                    .reviews
                    .get(&session_key)
                    .and_then(ReviewSessionAppState::persisted_snapshot)
                {
                    self.perform_delete_comment(session_key, comment_id, session);
                }
            }
        }
    }

    fn finish_error(&self, error: UiFailure, retry: Option<PendingAction>) {
        let authentication_message = error.authentication_reference.as_ref().map(|reference| {
            format!(
                "Sign in to {} for {}, then Pull Request Scanner will retry.",
                provider_label(reference),
                reference.host
            )
        });
        {
            let mut state = self.state.borrow_mut();
            state.main.is_opening = false;
            if let Some(review) = state.active_review_mut() {
                review.set_busy(false);
            }
            if let Some(authentication_message) = authentication_message {
                state
                    .main
                    .set_authentication_required(authentication_message);
                state.authentication_reference = error.authentication_reference;
                state.pending_action = retry;
                state.main.error_message.clear();
                state.main.status_message = error.message;
            } else {
                state.main.error_message = error.message;
            }
        }
        self.render();
    }

    fn render(&self) {
        let state = self.state.borrow();
        let has_error = state.main.has_error();
        let auth_required = state.main.authentication_required;
        let show_plain_status =
            !state.main.status_message.trim().is_empty() && !has_error && !auth_required;
        self.view
            .status
            .root
            .set_visible(auth_required || has_error || show_plain_status);
        self.view
            .status
            .auth_bar
            .set_message(&state.main.authentication_message);
        self.view.status.auth_bar.set_open(auth_required);
        self.view
            .status
            .authenticate
            .set_enabled(state.main.can_authenticate());
        self.view
            .status
            .error_bar
            .set_message(&state.main.error_message);
        self.view.status.error_bar.set_open(has_error);
        self.view.status.label.set_text(&state.main.status_message);
        self.view.status.label.set_visible(show_plain_status);

        self.view
            .review_shell
            .set_visible(!state.main.is_activity_view_visible);
        self.view.empty.set_visible(
            state.main.active_review_key.is_none() && !state.main.is_activity_view_visible,
        );
        self.view.open_review.panel.set_visible(
            state.main.is_review_panel_visible() && !state.main.is_activity_view_visible,
        );
        self.view
            .open_review
            .inline_open
            .set_enabled(state.main.can_open_review());
        self.view
            .open_review
            .sheet
            .set_button_enabled(DialogResponse::Primary, state.main.can_open_review());
        let _ = self
            .view
            .open_review
            .progress
            .SetIsActive(state.main.is_opening);
        raw::set_visible_control(&self.view.open_review.progress, state.main.is_opening);
        let _ = self
            .view
            .open_review
            .dialog_progress
            .SetIsActive(state.main.is_opening);
        raw::set_visible_control(
            &self.view.open_review.dialog_progress,
            state.main.is_opening,
        );
        self.view
            .open_review
            .inline_cancel
            .set_visible(state.main.active_review_key.is_some());
        self.view
            .activity
            .root
            .set_visible(state.main.is_activity_view_visible);
        self.view.workspace.root.set_visible(
            state.main.active_review_key.is_some() && !state.main.is_activity_view_visible,
        );
        self.render_sidebar(&state);
        self.render_toolbar(&state);
        if state.main.is_activity_view_visible {
            drop(state);
            self.render_activity();
        } else {
            drop(state);
            self.render_review();
        }
    }

    fn render_sidebar(&self, state: &ControllerState) {
        let active = state
            .main
            .active_reviews
            .iter()
            .map(|item| {
                let has_new = state
                    .reviews
                    .get(&item.session_key)
                    .is_some_and(ReviewSessionAppState::has_new_iteration);
                sidebar_active_item(item, has_new)
            })
            .collect::<Vec<_>>();
        let reviewed = state
            .main
            .reviewed_reviews
            .iter()
            .map(sidebar_reviewed_item)
            .collect::<Vec<_>>();
        self.view.sidebar.set_sections(&active, &reviewed);
        if state.main.is_activity_view_visible {
            self.view.sidebar.clear_selection();
            return;
        }
        if let Some(key) = state.main.active_review_key.as_deref() {
            let selected = state
                .main
                .active_reviews
                .iter()
                .position(|item| item.session_key.eq_ignore_ascii_case(key))
                .or_else(|| {
                    state
                        .main
                        .reviewed_reviews
                        .iter()
                        .position(|item| item.session_key.eq_ignore_ascii_case(key))
                        .map(|index| index + state.main.active_reviews.len())
                });
            if let Some(selected) = selected {
                self.view.sidebar.set_selected(selected);
            } else {
                self.view.sidebar.clear_selection();
            }
        } else {
            self.view.sidebar.clear_selection();
        }
    }

    fn render_toolbar(&self, state: &ControllerState) {
        let review = state.active_review();
        let readiness = review.map(ReviewSessionAppState::readiness);
        self.view
            .toolbar
            .set_item_enabled(0, !state.main.is_opening);
        self.view
            .toolbar
            .set_item_enabled(1, readiness.as_ref().is_some_and(|value| value.can_refresh));
        self.view.toolbar.set_item_enabled(
            3,
            readiness
                .as_ref()
                .is_some_and(|value| value.can_move_change_previous),
        );
        self.view.toolbar.set_item_enabled(
            4,
            readiness
                .as_ref()
                .is_some_and(|value| value.can_move_change_next),
        );
        self.view.toolbar.set_item_enabled(5, review.is_some());
        self.view.toolbar.set_item_enabled(
            7,
            readiness
                .as_ref()
                .is_some_and(|value| value.can_mark_reviewed),
        );
        self.view.toolbar.set_item_enabled(
            9,
            review.is_some_and(ReviewSessionAppState::has_description),
        );
    }

    fn render_review(&self) {
        let state = self.state.borrow();
        let Some(review) = state.active_review() else {
            return;
        };
        let resources = Resources::en_us();
        let files_label = resource(resources, "Review.Files").unwrap_or("Changed files");
        self.view
            .workspace
            .files
            .summary
            .set_text(&format!("{files_label} ({})", review.changed_file_count()));

        let primary_title = review.navigation_primary_title();
        self.view.workspace.title.set_text(&primary_title);
        self.view.workspace.title.set_tooltip(&primary_title);

        let subtitle = if let Some(definition) = review.definition_navigation.as_ref() {
            definition.heading.clone()
        } else if review.selected_file_path.is_empty() {
            review.author().to_string()
        } else {
            format!("{} · {}", review.author(), review.selected_file_path)
        };
        self.view.workspace.subtitle.set_text(&subtitle);
        self.view.workspace.subtitle.set_tooltip(&subtitle);

        let workspace_status = format_workspace_status(review);
        self.view
            .workspace
            .workspace_status
            .set_text(&workspace_status);
        self.view
            .workspace
            .new_iteration_badge
            .set_visible(review.has_new_iteration());

        self.view
            .workspace
            .progress
            .set_visible(review.is_busy || state.main.is_opening);
        let iteration_labels = review
            .iterations
            .iter()
            .map(|iteration| iteration.label.clone())
            .collect::<Vec<_>>();
        self.view.workspace.iteration.set_items(&iteration_labels);
        let selected_iteration = review
            .selected_iteration()
            .and_then(|selected| {
                review
                    .iterations
                    .iter()
                    .position(|iteration| iteration.id == selected.id)
            })
            .unwrap_or(0);
        self.view
            .workspace
            .iteration
            .set_selected(selected_iteration);
        self.view.workspace.iteration.set_enabled(!review.is_busy);

        let is_open = review.is_open();
        let can_reopen = review.session.as_ref().is_some_and(|session| {
            session.is_archived && !crate::state::is_provider_terminal(&session.details)
        });
        self.view.workspace.archive_menu.set_visible(is_open);
        self.view
            .workspace
            .archive_menu
            .set_enabled(!review.is_busy);
        self.view.workspace.reopen_menu.set_visible(can_reopen);
        self.view.workspace.reopen_menu.set_enabled(!review.is_busy);
        self.view
            .workspace
            .actions_separator
            .set_visible(is_open || can_reopen);
        self.view
            .workspace
            .complete_menu
            .set_enabled(!review.is_busy);

        self.view
            .workspace
            .diff
            .layout
            .set_selected(match review.diff.layout {
                DiffLayout::Inline => 0,
                DiffLayout::SideBySide => 1,
            });
        self.view.workspace.diff.context.set_selected(
            [3_u32, 5, 10, 20, 40]
                .iter()
                .position(|value| *value == review.context_lines)
                .unwrap_or(0),
        );
        self.view
            .workspace
            .diff
            .context_container
            .set_visible(review.diff.is_folder_selection);
        self.view
            .workspace
            .diff
            .back_to_diff
            .set_visible(review.is_viewing_definition);

        let has_folds = review.diff.has_function_folds();
        self.view.workspace.diff.collapse.set_visible(has_folds);
        self.view.workspace.diff.expand.set_visible(has_folds);
        self.view
            .workspace
            .diff
            .functions_separator
            .set_visible(has_folds);

        self.view
            .workspace
            .diff
            .status
            .set_text(&review.status_message);
        self.view
            .workspace
            .diff
            .status
            .set_visible(!review.status_message.trim().is_empty());

        self.view
            .workspace
            .inspector
            .error
            .set_message(&review.error_message);
        self.view
            .workspace
            .inspector
            .error
            .set_open(review.has_error());

        let provider_text = review
            .session
            .as_ref()
            .map(|session| {
                format!(
                    "{} · {} · {} changed file(s)",
                    provider_label(&session.details.reference),
                    review.author(),
                    review.changed_file_count()
                )
            })
            .unwrap_or_default();
        self.view
            .workspace
            .inspector
            .overview_provider
            .set_text(&provider_text);
        let nav_title = review.navigation_title();
        let web_url = review
            .session
            .as_ref()
            .map(|session| session.details.reference.web_url.clone())
            .unwrap_or_default();
        self.view
            .workspace
            .inspector
            .overview_link
            .set_text(&nav_title);
        self.view
            .workspace
            .inspector
            .overview_link
            .set_tooltip(&web_url);
        self.view
            .workspace
            .inspector
            .overview_link
            .set_visible(!web_url.trim().is_empty());
        let repo_source = review
            .session
            .as_ref()
            .map(|session| {
                if session.reused_existing_repository {
                    resource(resources, "Review.ReusingCheckout")
                        .unwrap_or("Reusing local checkout")
                } else {
                    resource(resources, "Review.UsingAppCache").unwrap_or("Using app cache")
                }
            })
            .unwrap_or_default();
        self.view
            .workspace
            .inspector
            .overview_repo_source
            .set_text(repo_source);
        let repo_location = review
            .session
            .as_ref()
            .map(|session| {
                session
                    .local_repository_path
                    .clone()
                    .unwrap_or_else(|| session.details.clone_url.clone())
            })
            .unwrap_or_default();
        self.view
            .workspace
            .inspector
            .overview_repo_location
            .set_text(&repo_location);

        self.view.workspace.inspector.notes.set_text(&review.notes);
        self.view
            .workspace
            .inspector
            .notes_status
            .set_text(match review.notes_save_state {
                NotesSaveState::Saved => resource(resources, "Notes.Saved").unwrap_or("Saved"),
                NotesSaveState::Saving => resource(resources, "Notes.Saving").unwrap_or("Saving"),
                NotesSaveState::Failed => {
                    resource(resources, "Notes.Failed").unwrap_or("Save failed")
                }
            });
        self.view
            .workspace
            .inspector
            .comments
            .current_file
            .set_checked(review.show_only_current_file_comments);
        self.view
            .workspace
            .inspector
            .comments
            .hide_bots
            .set_checked(review.hide_bot_comments);
        self.view
            .workspace
            .inspector
            .comments
            .draft
            .set_text(&review.draft_comment);
        self.view
            .workspace
            .inspector
            .comments
            .suggestion
            .set_text(&review.draft_suggestion);
        self.apply_selected_line_ui(review);
        let pending_count = review.pending_comment_count();
        self.view
            .workspace
            .inspector
            .comments
            .pending_summary
            .set_text(&format_pending_summary(pending_count));
        self.view
            .workspace
            .inspector
            .comments
            .pending_summary
            .set_visible(pending_count > 0);
        self.view
            .workspace
            .inspector
            .comments
            .publish
            .set_text(&format_publish_button_label(pending_count));
        self.view
            .workspace
            .inspector
            .comments
            .publish
            .set_enabled(review.can_publish_pending_comments());
        self.view
            .workspace
            .inspector
            .tabs
            .set_selected(match review.inspector_tab {
                crate::state::InspectorTab::Comments => 0,
                crate::state::InspectorTab::Overview => 1,
                crate::state::InspectorTab::Notes => 2,
            });
        drop(state);
        self.render_file_tree();
        self.render_diff();
        self.render_comments();
        self.render_focus_layout();
    }

    fn render_file_tree(&self) {
        let Some((items, selected)) = ({
            let state = self.state.borrow();
            state.active_review().map(|review| {
                (
                    review.file_roots.iter().map(tree_item).collect::<Vec<_>>(),
                    file_node_path(&review.file_roots, &review.selected_file_path),
                )
            })
        }) else {
            self.view.workspace.files.tree.set_items(&[]);
            return;
        };
        self.view.workspace.files.tree.set_items(&items);
        if let Some(selected) = selected {
            self.view.workspace.files.tree.set_selected(&selected);
        }
    }

    fn render_diff(&self) {
        let snapshot = {
            let state = self.state.borrow();
            state.active_review().map(|review| {
                let readiness = review.readiness();
                let search_position = if review.diff.has_search_query() {
                    if let Some(position) = review.diff.search_position() {
                        format!("{position} / {}", review.diff.search_match_indices.len())
                    } else {
                        "0 / 0".to_string()
                    }
                } else {
                    String::new()
                };
                (
                    review.diff.rows.clone(),
                    review.diff.selected_line_index,
                    review.is_markdown_file(),
                    review.is_formatted_markdown_visible(),
                    review.diff.change_markers.clone(),
                    readiness,
                    search_position,
                    review.diff.has_function_folds(),
                    format_workspace_status(review),
                )
            })
        };
        let Some((
            rows,
            selected_line,
            is_markdown,
            is_formatted,
            markers,
            readiness,
            search_position,
            has_folds,
            workspace_status,
        )) = snapshot
        else {
            return;
        };
        self.view
            .workspace
            .workspace_status
            .set_text(&workspace_status);
        self.view
            .workspace
            .diff
            .search
            .set_enabled(readiness.can_search_file);
        self.view
            .workspace
            .diff
            .search_position
            .set_text(&search_position);
        self.view
            .workspace
            .diff
            .search_position
            .set_visible(!search_position.is_empty());
        self.view
            .workspace
            .diff
            .previous_search
            .set_enabled(readiness.can_move_search_previous);
        self.view
            .workspace
            .diff
            .next_search
            .set_enabled(readiness.can_move_search_next);
        self.view
            .toolbar
            .set_item_enabled(3, readiness.can_move_change_previous);
        self.view
            .toolbar
            .set_item_enabled(4, readiness.can_move_change_next);
        self.view.workspace.diff.collapse.set_visible(has_folds);
        self.view.workspace.diff.expand.set_visible(has_folds);
        self.view
            .workspace
            .diff
            .functions_separator
            .set_visible(has_folds);
        self.view.workspace.diff.raw.set_visible(!is_formatted);
        self.view
            .workspace
            .diff
            .markdown_scroll
            .set_visible(is_formatted);
        self.view
            .workspace
            .diff
            .markdown_container
            .set_visible(is_markdown);
        self.view
            .workspace
            .diff
            .markdown_toggle
            .set_text(if is_formatted {
                resource(Resources::en_us(), "Review.SourceView").unwrap_or("View source")
            } else {
                resource(Resources::en_us(), "Review.MarkdownToggle").unwrap_or("View formatted")
            });
        if is_formatted {
            self.render_markdown();
            return;
        }
        let show_markers = !markers.is_empty();
        self.view
            .workspace
            .diff
            .marker_host
            .set_visible(show_markers);
        let _ = self.view.workspace.diff.marker_column.SetWidth(GridLength {
            Value: if show_markers { 24.0 } else { 0.0 },
            GridUnitType: GridUnitType::Pixel,
        });
        let side_by_side_width = rows
            .iter()
            .any(crate::state::DiffRowState::is_side_by_side)
            .then_some(640.0);
        self.view
            .workspace
            .diff
            .table
            .set_side_by_side_min_width(side_by_side_width);
        let weak = self.weak_self();
        let ui = self.ui.clone();
        self.view
            .workspace
            .diff
            .table
            .set_row_builder(rows.len(), move |index| {
                build_diff_row(&ui, weak.clone(), index)
            });
        if let Some(selected_line) = selected_line
            && let Some(row_index) = rows
                .iter()
                .position(|row| row.contains_source_index(selected_line))
        {
            self.view.workspace.diff.table.set_selected(row_index);
            self.view.workspace.diff.table.scroll_to_row(row_index);
        }
        self.view.workspace.diff.marker.on_draw(move |painter| {
            draw_diff_markers(painter, &markers);
        });
        self.view.workspace.diff.marker.redraw();
    }

    fn render_markdown(&self) {
        let blocks = {
            self.state
                .borrow()
                .active_review()
                .map(|review| review.markdown_blocks.clone())
                .unwrap_or_default()
        };
        let content = &self.view.workspace.diff.markdown_content;
        content.clear();
        let resources = Resources::en_us();
        let comment_name = resource(resources, "Review.MarkdownBlockCommentName")
            .unwrap_or("Comment on this Markdown block");
        let comment_tooltip = resource(resources, "Review.MarkdownBlockCommentTooltip")
            .unwrap_or("Add a comment to this Markdown block");
        for block in blocks {
            match build_markdown_file_row(
                self.ui.tasks(),
                self.weak_self(),
                &block,
                comment_name,
                comment_tooltip,
            ) {
                Ok(row) => content.append_element(&row),
                Err(error) => self.show_local_ui_error(error),
            }
        }
    }

    fn show_comment_for_source_line(&self, line_number: usize) {
        let selected = {
            let mut state = self.state.borrow_mut();
            let Some(review) = state.active_review_mut() else {
                return;
            };
            let index = review
                .diff
                .lines
                .iter()
                .find(|line| {
                    line.line.new_line_number == Some(line_number)
                        || line.line.old_line_number == Some(line_number)
                })
                .map(|line| line.source_index);
            if let Some(index) = index {
                review.select_diff_line(index);
                true
            } else {
                false
            }
        };
        if selected {
            self.show_focused_comment();
        }
    }

    fn render_comments(&self) {
        let state = self.state.borrow();
        let Some(review) = state.active_review() else {
            return;
        };
        let local = review
            .visible_comments()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let remote = review
            .visible_remote_threads()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        self.apply_selected_line_ui(review);
        drop(state);
        let comments = &self.view.workspace.inspector.comments;
        comments.list.clear();
        comments.remote_list.clear();
        comments.remote_panel.set_visible(!remote.is_empty());
        comments
            .empty
            .set_visible(local.is_empty() && remote.is_empty());
        for comment in local {
            match self.local_comment_card(&comment) {
                Ok(card) => comments.list.append(&card),
                Err(error) => self.show_local_ui_error(error),
            }
        }
        for thread in remote {
            match self.remote_thread_card(&thread) {
                Ok(card) => comments.remote_list.append(&card),
                Err(error) => self.show_local_ui_error(error),
            }
        }
    }

    fn local_comment_card(&self, comment: &crate::state::LocalReviewCommentState) -> Result<Pane> {
        let root = raw::load::<XamlGrid>(COMMENT_CARD_XAML)?
            .cast::<FrameworkElement>()
            .map_err(|error| naui::Error::new("get a comment card", error.to_string()))?;
        let comment_id = comment.id().to_string();
        set_xaml_text(&root, "AuthorText", comment.author())?;
        set_xaml_text(&root, "LocationText", &comment.location())?;
        set_xaml_text(&root, "BodyText", &comment.display_body())?;
        set_xaml_text(
            &root,
            "SelectedCodeHeader",
            resource(Resources::en_us(), "Review.SelectedCodeCard")?,
        )?;
        set_xaml_text(
            &root,
            "SuggestionHeader",
            resource(Resources::en_us(), "Review.Suggestion")?,
        )?;
        set_xaml_text(&root, "SelectedCodeText", &comment.comment.selected_text)?;
        set_xaml_text(
            &root,
            "SuggestionText",
            &comment.comment.suggested_replacement,
        )?;
        set_comment_expansion(&root, comment.is_expanded)?;

        let resources = Resources::en_us();
        let status = match comment.comment.status {
            ReviewCommentStatus::Pending => resource(resources, "Review.StatusDraft")?,
            ReviewCommentStatus::Failed => resource(resources, "Review.StatusFailed")?,
            ReviewCommentStatus::Published => resource(resources, "Review.StatusPublished")?,
        };
        set_xaml_visible(
            &root,
            "DraftBadge",
            comment.comment.status == ReviewCommentStatus::Pending,
        )?;
        set_xaml_text(&root, "DraftStatusText", status)?;
        set_xaml_visible(
            &root,
            "FailedBadge",
            comment.comment.status == ReviewCommentStatus::Failed,
        )?;
        set_xaml_text(&root, "FailedStatusText", status)?;
        set_xaml_visible(
            &root,
            "PublishedBadge",
            comment.comment.status == ReviewCommentStatus::Published,
        )?;
        set_xaml_text(&root, "PublishedStatusText", status)?;
        set_xaml_visible(&root, "ActiveBadge", false)?;
        set_xaml_visible(&root, "ResolvedBadge", false)?;

        let has_selection = !comment.comment.selected_text.trim().is_empty();
        let has_suggestion = !comment.comment.suggested_replacement.trim().is_empty();
        let error = comment.comment.error.as_deref().unwrap_or_default();
        set_xaml_visible(&root, "BodyText", !comment.is_editing)?;
        set_xaml_visible(
            &root,
            "SelectionPanel",
            !comment.is_editing && has_selection,
        )?;
        set_xaml_visible(
            &root,
            "SuggestionPanel",
            !comment.is_editing && has_suggestion,
        )?;
        set_xaml_visible(
            &root,
            "ErrorText",
            !comment.is_editing && !error.trim().is_empty(),
        )?;
        set_xaml_text(&root, "ErrorText", error)?;
        set_xaml_visible(&root, "EditComposer", comment.is_editing)?;
        set_xaml_visible(&root, "ReplyComposer", false)?;
        set_xaml_visible(&root, "BeginReplyButton", false)?;
        set_xaml_visible(&root, "JumpToCommentButton", false)?;
        set_xaml_visible(&root, "OpenProviderButton", false)?;

        let toggle = button(self.ui.tasks(), &root, "ToggleThreadButton")?;
        let toggle_label = resource(resources, "Review.ToggleThreadName")?;
        toggle.set_accessible_label(toggle_label);
        toggle.set_tooltip(toggle_label);
        toggle.on_click({
            let weak = self.weak_self();
            let comment_id = comment_id.clone();
            move || {
                if let Some(app) = weak.upgrade() {
                    if let Some(review) = app.state.borrow_mut().active_review_mut()
                        && let Some(comment) = review
                            .comments
                            .iter_mut()
                            .find(|item| item.id().eq_ignore_ascii_case(&comment_id))
                    {
                        comment.toggle_expanded();
                    }
                    app.render_comments();
                    app.render_diff();
                }
            }
        });

        let edit = button(self.ui.tasks(), &root, "EditCommentButton")?;
        let edit_label = resource(resources, "Review.Edit")?;
        edit.set_accessible_label(edit_label);
        edit.set_tooltip(edit_label);
        edit.set_visible(comment.can_edit() && !comment.is_editing);
        edit.on_click({
            let weak = self.weak_self();
            let comment_id = comment_id.clone();
            move || {
                if let Some(app) = weak.upgrade() {
                    if let Some(review) = app.state.borrow_mut().active_review_mut() {
                        let _ = review.begin_comment_edit(&comment_id);
                    }
                    app.render_comments();
                    app.render_diff();
                }
            }
        });
        let delete = button(self.ui.tasks(), &root, "DeleteCommentButton")?;
        let delete_label = resource(resources, "Review.Delete")?;
        delete.set_accessible_label(delete_label);
        delete.set_tooltip(delete_label);
        delete.set_visible(true);
        delete.on_click({
            let weak = self.weak_self();
            let comment_id = comment_id.clone();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.request_delete_comment(comment_id.clone());
                }
            }
        });

        let edit_body = text_input(self.ui.tasks(), &root, "EditBodyTextBox")?;
        edit_body.set_header(resource(resources, "Review.EditCommentHeader")?);
        edit_body.set_placeholder(resource(resources, "Review.CommentPlaceholder")?);
        edit_body.set_accessible_label(resource(resources, "Review.EditCommentName")?);
        edit_body.set_text(&comment.edit_body);
        let edit_suggestion = text_input(self.ui.tasks(), &root, "EditSuggestionTextBox")?;
        edit_suggestion.set_header(resource(resources, "Review.SuggestionHeader")?);
        edit_suggestion.set_placeholder(resource(resources, "Review.SuggestionPlaceholder")?);
        edit_suggestion.set_accessible_label(resource(resources, "Review.SuggestionName")?);
        edit_suggestion.set_text(&comment.edit_suggestion);
        let save_edit = button(self.ui.tasks(), &root, "SaveEditButton")?;
        save_edit.set_text(resource(Resources::en_us(), "Review.SaveEdit")?);
        save_edit.set_enabled(comment.can_save_edit());
        edit_body.on_change({
            let weak = self.weak_self();
            let comment_id = comment_id.clone();
            let save_edit = save_edit.clone();
            move |text| {
                if let Some(app) = weak.upgrade()
                    && let Some(review) = app.state.borrow_mut().active_review_mut()
                    && let Some(item) = review
                        .comments
                        .iter_mut()
                        .find(|item| item.id().eq_ignore_ascii_case(&comment_id))
                {
                    item.edit_body = text.to_string();
                    save_edit.set_enabled(item.can_save_edit());
                }
            }
        });
        edit_suggestion.on_change({
            let weak = self.weak_self();
            let comment_id = comment_id.clone();
            let save_edit = save_edit.clone();
            move |text| {
                if let Some(app) = weak.upgrade()
                    && let Some(review) = app.state.borrow_mut().active_review_mut()
                    && let Some(item) = review
                        .comments
                        .iter_mut()
                        .find(|item| item.id().eq_ignore_ascii_case(&comment_id))
                {
                    item.edit_suggestion = text.to_string();
                    save_edit.set_enabled(item.can_save_edit());
                }
            }
        });
        save_edit.on_click({
            let weak = self.weak_self();
            let comment_id = comment_id.clone();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.commit_comment_edit(comment_id.clone());
                }
            }
        });
        let cancel_edit = button(self.ui.tasks(), &root, "CancelEditButton")?;
        cancel_edit.set_text(resource(Resources::en_us(), "Review.Cancel")?);
        cancel_edit.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    if let Some(review) = app.state.borrow_mut().active_review_mut() {
                        let _ = review.cancel_comment_edit(&comment_id);
                    }
                    app.render_comments();
                    app.render_diff();
                }
            }
        });

        Ok(Pane::from_element(raw::ui_element(&root)?))
    }

    fn remote_thread_card(&self, thread: &crate::state::RemoteReviewThreadState) -> Result<Pane> {
        let root = raw::load::<XamlGrid>(COMMENT_CARD_XAML)?
            .cast::<FrameworkElement>()
            .map_err(|error| naui::Error::new("get a comment thread card", error.to_string()))?;
        let thread_id = thread.id.clone();
        set_xaml_text(&root, "AuthorText", &thread.root.comment.author)?;
        set_xaml_text(&root, "LocationText", &thread.root.location())?;
        set_xaml_text(&root, "BodyText", &thread.root.display_body())?;
        set_xaml_visible(&root, "DraftBadge", false)?;
        set_xaml_visible(&root, "FailedBadge", false)?;
        set_xaml_visible(&root, "PublishedBadge", false)?;
        let resources = Resources::en_us();
        set_xaml_visible(&root, "ActiveBadge", !thread.root.comment.is_resolved)?;
        set_xaml_text(
            &root,
            "ActiveStatusText",
            resource(resources, "Review.StatusActive")?,
        )?;
        set_xaml_visible(&root, "ResolvedBadge", thread.root.comment.is_resolved)?;
        set_xaml_text(
            &root,
            "ResolvedStatusText",
            resource(resources, "Review.StatusResolved")?,
        )?;
        set_comment_expansion(&root, thread.is_expanded)?;

        let has_replies = thread.has_replies();
        set_xaml_visible(&root, "ThreadSummaryText", has_replies)?;
        let thread_summary = format!("{} comment(s)", thread.thread_comment_count());
        set_xaml_text(
            &root,
            "ThreadSummaryText",
            if has_replies { &thread_summary } else { "" },
        )?;
        set_xaml_visible(&root, "SelectionPanel", false)?;
        set_xaml_visible(&root, "SuggestionPanel", false)?;
        set_xaml_visible(&root, "ErrorText", false)?;
        set_xaml_visible(&root, "EditComposer", false)?;
        set_xaml_visible(&root, "EditCommentButton", false)?;
        set_xaml_visible(&root, "DeleteCommentButton", false)?;

        let toggle = button(self.ui.tasks(), &root, "ToggleThreadButton")?;
        let toggle_label = resource(resources, "Review.ToggleThreadName")?;
        toggle.set_accessible_label(toggle_label);
        toggle.set_tooltip(toggle_label);
        toggle.on_click({
            let weak = self.weak_self();
            let thread_id = thread_id.clone();
            move || {
                if let Some(app) = weak.upgrade() {
                    if let Some(review) = app.state.borrow_mut().active_review_mut()
                        && let Some(thread) = review
                            .remote_threads
                            .iter_mut()
                            .find(|item| item.id.eq_ignore_ascii_case(&thread_id))
                    {
                        thread.is_expanded = !thread.is_expanded;
                    }
                    app.render_comments();
                    app.render_diff();
                }
            }
        });

        let jump = button(self.ui.tasks(), &root, "JumpToCommentButton")?;
        let jump_label = resource(resources, "Review.JumpToLine")?;
        jump.set_accessible_label(jump_label);
        jump.set_tooltip(jump_label);
        jump.set_visible(thread.root.show_in_app_jump());
        jump.on_click({
            let weak = self.weak_self();
            let comment = thread.root.comment.clone();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.jump_to_comment(comment.clone());
                }
            }
        });
        let provider = button(self.ui.tasks(), &root, "OpenProviderButton")?;
        let provider_label = resource(resources, "Review.OpenProvider")?;
        provider.set_accessible_label(provider_label);
        provider.set_tooltip(provider_label);
        let show_provider =
            !thread.root.comment.web_url.trim().is_empty() && !thread.root.show_in_app_jump();
        provider.set_visible(show_provider);
        provider.on_click({
            let ui = self.ui.clone();
            let url = thread.root.comment.web_url.clone();
            move || {
                let _ = ui.open_url(&url);
            }
        });

        let replies = raw::element::<XamlStackPanel>(&root, "RepliesList")?;
        for reply in &thread.replies {
            let reply_root = raw::load::<XamlGrid>(COMMENT_REPLY_XAML)?
                .cast::<FrameworkElement>()
                .map_err(|error| naui::Error::new("get a comment reply", error.to_string()))?;
            set_xaml_text(&reply_root, "AuthorText", &reply.comment.author)?;
            set_xaml_text(&reply_root, "BodyText", &reply.display_body())?;
            append_xaml_panel_child(&replies, &reply_root)?;
        }

        let begin_reply = button(self.ui.tasks(), &root, "BeginReplyButton")?;
        begin_reply.set_text(resource(Resources::en_us(), "Review.BeginReply")?);
        begin_reply.set_visible(!thread.is_reply_composer_visible);
        begin_reply.on_click({
            let weak = self.weak_self();
            let thread_id = thread_id.clone();
            move || {
                if let Some(app) = weak.upgrade() {
                    if let Some(review) = app.state.borrow_mut().active_review_mut() {
                        let _ = review.begin_remote_reply(&thread_id);
                    }
                    app.render_comments();
                    app.render_diff();
                }
            }
        });
        set_xaml_visible(&root, "ReplyComposer", thread.is_reply_composer_visible)?;
        let reply_body = text_input(self.ui.tasks(), &root, "ReplyTextBox")?;
        reply_body.set_text(&thread.reply_text);
        reply_body.set_placeholder(resource(resources, "Review.ReplyPlaceholder")?);
        reply_body.set_accessible_label(resource(resources, "Review.ReplyName")?);
        let post_reply = button(self.ui.tasks(), &root, "PostReplyButton")?;
        post_reply.set_text(resource(Resources::en_us(), "Review.PostReply")?);
        post_reply.set_enabled(thread.can_submit_reply());
        reply_body.on_change({
            let weak = self.weak_self();
            let thread_id = thread_id.clone();
            let post_reply = post_reply.clone();
            move |text| {
                if let Some(app) = weak.upgrade()
                    && let Some(review) = app.state.borrow_mut().active_review_mut()
                    && let Some(thread) = review
                        .remote_threads
                        .iter_mut()
                        .find(|item| item.id.eq_ignore_ascii_case(&thread_id))
                {
                    thread.reply_text = text.to_string();
                    post_reply.set_enabled(thread.can_submit_reply());
                }
            }
        });
        post_reply.on_click({
            let weak = self.weak_self();
            let thread_id = thread_id.clone();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.post_reply(&thread_id);
                }
            }
        });
        let cancel_reply = button(self.ui.tasks(), &root, "CancelReplyButton")?;
        cancel_reply.set_text(resource(Resources::en_us(), "Review.CancelReply")?);
        cancel_reply.on_click({
            let weak = self.weak_self();
            move || {
                if let Some(app) = weak.upgrade() {
                    if let Some(review) = app.state.borrow_mut().active_review_mut() {
                        let _ = review.cancel_remote_reply(&thread_id);
                    }
                    app.render_comments();
                    app.render_diff();
                }
            }
        });

        Ok(Pane::from_element(raw::ui_element(&root)?))
    }

    fn post_reply(&self, thread_id: &str) {
        let request = {
            let state = self.state.borrow();
            let Some(review) = state.active_review() else {
                return;
            };
            let Some(thread) = review
                .remote_threads
                .iter()
                .find(|thread| thread.id.eq_ignore_ascii_case(thread_id))
            else {
                return;
            };
            if !review.can_submit_remote_reply(thread_id) {
                return;
            }
            let Some(session) = review.persisted_snapshot() else {
                return;
            };
            (
                session.details.reference.session_key.clone(),
                thread.root.comment.clone(),
                thread.reply_text.clone(),
            )
        };
        self.reply_to_comment(request.0, request.1, request.2);
    }

    fn reply_to_comment(&self, session_key: String, parent: RemoteReviewComment, body: String) {
        let Some(mut session) = self
            .state
            .borrow()
            .reviews
            .get(&session_key)
            .and_then(ReviewSessionAppState::persisted_snapshot)
        else {
            return;
        };
        let services = self.services.clone();
        let retry = PendingAction::ReplyToComment {
            session_key: session_key.clone(),
            comment: Box::new(parent.clone()),
            body: body.clone(),
        };
        self.spawn(async move {
            let result = services
                .reply_to_comment(&mut session, &parent, &body, None)
                .await
                .map_err(UiFailure::from);
            Box::new(move |app: &AppController| {
                app.finish_reply(session_key, session, result, retry)
            }) as UiEvent
        });
    }

    fn finish_reply(
        &self,
        session_key: String,
        session: ReviewSessionState,
        result: std::result::Result<String, UiFailure>,
        retry: PendingAction,
    ) {
        match result {
            Ok(_) => {
                if let Some(review) = self.state.borrow_mut().reviews.get_mut(&session_key) {
                    review.replace_session_state_preserving_view(&session);
                    review.set_status("Reply posted.");
                }
                self.render();
            }
            Err(error) => self.finish_error(error, Some(retry)),
        }
    }

    fn jump_to_comment(&self, comment: RemoteReviewComment) {
        let Some(path) = (!comment.file_path.trim().is_empty()).then(|| comment.file_path.clone())
        else {
            return;
        };
        {
            let mut state = self.state.borrow_mut();
            if let Some(review) = state.active_review_mut() {
                review.set_file_filter("");
                let _ = review.select_file_or_folder(&path);
                let session_key = review
                    .session
                    .as_ref()
                    .map(|session| session.details.reference.session_key.clone());
                if let Some(session_key) = session_key {
                    state.pending_comment_jump = Some(PendingLineJump {
                        session_key,
                        path: path.clone(),
                        line_number: comment.line_number,
                        side: comment.side,
                    });
                }
            }
        }
        self.render();
        self.load_diff_for_path(path, 3, false);
    }

    fn show_description(&self) {
        let blocks = {
            let state = self.state.borrow();
            state.active_review().map(|review| {
                if !review.description_markdown_blocks.is_empty() {
                    review.description_markdown_blocks.clone()
                } else {
                    review
                        .session
                        .as_ref()
                        .map(|session| prs_core::markdown::parse(&session.details.description))
                        .unwrap_or_default()
                }
            })
        };
        let Some(blocks) = blocks else {
            return;
        };
        self.view.description.content.clear();
        for block in &blocks {
            if let Ok(element) = build_markdown_body_element(block) {
                self.view.description.content.append_element(&element);
            }
        }
        self.view.description.sheet.open();
    }

    fn apply_selected_line_ui(&self, review: &ReviewSessionAppState) {
        let line_description = format_selected_line_description(review);
        let fallback_hint = resource(Resources::en_us(), "Review.SelectLineHint")
            .unwrap_or("Select a diff line to leave a review comment.");
        self.view
            .workspace
            .inspector
            .comments
            .selected_line
            .set_text(if line_description.is_empty() {
                fallback_hint
            } else {
                &line_description
            });
        let has_fragment = !review.selected_code_fragment.trim().is_empty();
        self.view
            .workspace
            .inspector
            .comments
            .selected_code_panel
            .set_visible(has_fragment);
        self.view
            .workspace
            .inspector
            .comments
            .selected_code
            .set_text(&review.selected_code_fragment);
        self.view
            .workspace
            .inspector
            .comments
            .save_comment
            .set_enabled(review.can_save_comment());
        let workspace_status = format_workspace_status(review);
        self.view
            .workspace
            .workspace_status
            .set_text(&workspace_status);
    }

    fn select_diff_row(&self, row_index: usize) {
        let changed = {
            let mut state = self.state.borrow_mut();
            let Some(review) = state.active_review_mut() else {
                return;
            };
            let Some(row) = review.diff.rows.get(row_index) else {
                return;
            };
            let Some(source_index) = row
                .inline
                .as_ref()
                .or(row.hunk.as_ref())
                .or(row.right.as_ref())
                .or(row.left.as_ref())
                .map(|line| line.source_index)
            else {
                return;
            };
            if review.diff.selected_line_index == Some(source_index) {
                self.apply_selected_line_ui(review);
                false
            } else {
                review.select_diff_line(source_index)
            }
        };
        if changed {
            self.render_comments();
            self.render_diff();
        }
    }

    fn navigate_change(&self, direction: isize) {
        let changed = {
            let mut state = self.state.borrow_mut();
            state
                .active_review_mut()
                .and_then(|review| review.navigate_to_change(direction))
                .is_some()
        };
        if changed {
            self.render();
        }
    }

    fn navigate_to_marker(&self, y: f64) {
        let (_, height) = self.view.workspace.diff.marker.size();
        if height <= 0.0 {
            return;
        }
        let relative = (y / height).clamp(0.0, 1.0);
        let selected = {
            let mut state = self.state.borrow_mut();
            let Some(review) = state.active_review_mut() else {
                return;
            };
            let nearest = review
                .diff
                .change_markers
                .iter()
                .min_by(|left, right| {
                    (left.relative_position - relative)
                        .abs()
                        .total_cmp(&(right.relative_position - relative).abs())
                })
                .map(|marker| marker.line_index);
            nearest.is_some_and(|line_index| review.select_diff_line(line_index))
        };
        if selected {
            self.render_diff();
            self.render_comments();
        }
    }

    fn navigate_search(&self, direction: isize) {
        let changed = {
            let mut state = self.state.borrow_mut();
            state
                .active_review_mut()
                .and_then(|review| review.navigate_search(direction))
                .is_some()
        };
        if changed {
            self.render_diff();
        }
    }

    fn navigate_definition(&self) {
        let request = {
            let state = self.state.borrow();
            let Some(review) = state.active_review() else {
                return;
            };
            let Some(line) = review.selected_diff_line() else {
                return;
            };
            let Some(symbol_link) = line.primary_navigation_symbol() else {
                return;
            };
            let symbol = symbol_link.symbol.clone();
            let Some(snapshot) = review.persisted_snapshot() else {
                return;
            };
            let Some(range) = review.active_range() else {
                return;
            };
            let commit = if line.line.kind == DiffLineKind::Deleted {
                range.base_commit_sha
            } else {
                range.head_commit_sha
            };
            let path = line.line.file_path.clone();
            let line_number = line.line.new_line_number.or(line.line.old_line_number);
            Some((symbol, snapshot, commit, path, line_number))
        };
        let Some((symbol, session, commit, path, source_line)) = request else {
            return;
        };
        let session_key = session.details.reference.session_key.clone();
        let services = self.services.clone();
        self.spawn(async move {
            let result = async {
                let location = services
                    .find_definition(&session, &symbol, &path, Some(&commit), source_line, None)
                    .await?
                    .ok_or_else(|| {
                        crate::service::ReviewSessionServiceError::Core(
                            prs_core::CoreError::operation(format!(
                                "No definition was found for '{symbol}'."
                            )),
                        )
                    })?;
                let content = services
                    .read_file(&session, &location.file_path, Some(&commit), None)
                    .await?;
                Ok::<_, crate::service::ReviewSessionServiceError>((symbol, location, content))
            }
            .await
            .map_err(UiFailure::from);
            Box::new(move |app: &AppController| app.finish_definition(session_key, result))
                as UiEvent
        });
    }

    fn finish_definition(
        &self,
        session_key: String,
        result: std::result::Result<(String, prs_core::SymbolLocation, String), UiFailure>,
    ) {
        match result {
            Ok((symbol, location, content)) => {
                if let Some(review) = self.state.borrow_mut().reviews.get_mut(&session_key)
                    && !review.show_definition(symbol, location, &content)
                {
                    review.set_error("The definition is outside the available source.");
                }
                self.render();
            }
            Err(error) => self.finish_error(error, None),
        }
    }

    fn render_activity(&self) {
        let state = self.state.borrow();
        let activity = &state.activity;
        self.view.activity.month.set_text(&activity.month_label());
        self.view
            .activity
            .pull_request_total
            .set_text(&activity.pull_request_total.to_string());
        self.view
            .activity
            .comment_total
            .set_text(&activity.comment_total.to_string());
        self.view
            .activity
            .review_total
            .set_text(&activity.review_total.to_string());
        self.view
            .activity
            .previous_month
            .set_enabled(activity.can_move_to_previous_month());
        self.view
            .activity
            .next_month
            .set_enabled(activity.can_move_to_next_month());
        let _ = self.view.activity.progress.SetIsActive(activity.is_busy);
        raw::set_visible_control(&self.view.activity.progress, activity.is_busy);
        let has_error = !activity.error_message.trim().is_empty();
        self.view
            .activity
            .error
            .set_message(&activity.error_message);
        self.view.activity.error.set_open(has_error);
        let rows = activity
            .recent_page()
            .iter()
            .map(|review| {
                TableRow::new([
                    review.pull_request_title.clone(),
                    review.location(),
                    review.date.to_string(),
                ])
            })
            .collect::<Vec<_>>();
        self.view.activity.recent.set_rows(&rows);
        self.view.activity.recent_host.set_visible(!rows.is_empty());
        self.view.activity.empty.set_visible(rows.is_empty());
        let days = activity.days.clone();
        let maximum = activity.chart_maximum;
        drop(state);
        self.view.activity.chart.on_draw(move |painter| {
            draw_activity_chart(painter, &days, maximum);
        });
        self.view.activity.chart.redraw();
    }

    fn render_notes(&self) {
        let state = self.state.borrow();
        if let Some(review) = state.active_review() {
            self.view
                .workspace
                .inspector
                .notes_status
                .set_text(match review.notes_save_state {
                    NotesSaveState::Saved => {
                        resource(Resources::en_us(), "Notes.Saved").unwrap_or("Saved")
                    }
                    NotesSaveState::Saving => {
                        resource(Resources::en_us(), "Notes.Saving").unwrap_or("Saving")
                    }
                    NotesSaveState::Failed => {
                        resource(Resources::en_us(), "Notes.Failed").unwrap_or("Save failed")
                    }
                });
        }
    }

    fn render_focus_layout(&self) {
        let (expanded, files_width, splitter_width, inspector_width) = {
            let state = self.state.borrow();
            (
                state.main.is_code_view_expanded,
                state.main.changed_files_pane_width_value(),
                state.main.pane_splitter_width(),
                state.main.review_inspector_pane_width_value(),
            )
        };
        let pixel = |value: f64| GridLength {
            Value: value,
            GridUnitType: GridUnitType::Pixel,
        };
        let _ = self
            .view
            .workspace
            .files_column
            .SetWidth(pixel(files_width));
        let _ = self
            .view
            .workspace
            .files_splitter_column
            .SetWidth(pixel(splitter_width));
        let _ = self
            .view
            .workspace
            .inspector_splitter_column
            .SetWidth(pixel(splitter_width));
        let _ = self
            .view
            .workspace
            .inspector_column
            .SetWidth(pixel(inspector_width));
        self.view.workspace.files.root.set_visible(!expanded);
        self.view.workspace.files_splitter.set_visible(!expanded);
        self.view
            .workspace
            .inspector_splitter
            .set_visible(!expanded);
        self.view.workspace.inspector.root.set_visible(!expanded);
        let _ = self
            .view
            .workspace
            .focus_glyph
            .SetGlyph(&HSTRING::from(if expanded {
                "\u{E73F}"
            } else {
                "\u{E740}"
            }));
        let tooltip = if expanded {
            resource(Resources::en_us(), "App.ExitFocus").unwrap_or("Exit focus")
        } else {
            resource(Resources::en_us(), "App.Focus").unwrap_or("Focus")
        };
        self.view.workspace.focus_button.set_tooltip(tooltip);
        self.view.sidebar.set_collapsed(expanded);
    }

    fn show_local_ui_error(&self, error: naui::Error) {
        let message = error.to_string();
        self.state.borrow_mut().main.error_message = message.clone();
        self.view.status.root.set_visible(true);
        self.view.status.error_bar.set_message(&message);
        self.view.status.error_bar.set_open(true);
        self.view.status.label.set_visible(false);
    }
}

fn sidebar_active_item(
    item: &crate::state::ReviewNavigationItemState,
    has_new_iteration: bool,
) -> SidebarReviewItem {
    let badge = has_new_iteration.then(|| {
        resource(Resources::en_us(), "Review.NewIterationShort")
            .unwrap_or("New")
            .to_string()
    });
    let tooltip = if has_new_iteration {
        format!(
            "{}\n{}",
            item.navigation_tooltip(),
            resource(Resources::en_us(), "Review.NewIteration").unwrap_or("New iteration")
        )
    } else {
        item.navigation_tooltip()
    };
    SidebarReviewItem {
        primary: item.navigation_primary_title(),
        secondary: item.navigation_secondary_title(),
        tooltip,
        badge,
        status: None,
    }
}

fn sidebar_reviewed_item(item: &crate::state::ReviewNavigationItemState) -> SidebarReviewItem {
    let resources = Resources::en_us();
    let status = match item.status {
        ReviewNavigationStatus::Merged => {
            resource(resources, "Review.StatusMerged").unwrap_or("Merged")
        }
        ReviewNavigationStatus::Closed => {
            resource(resources, "Review.StatusClosed").unwrap_or("Closed")
        }
        ReviewNavigationStatus::Completed => {
            resource(resources, "Review.StatusCompleted").unwrap_or("Completed")
        }
        ReviewNavigationStatus::Archived | ReviewNavigationStatus::Open => {
            resource(resources, "Review.StatusReviewed").unwrap_or("Reviewed")
        }
    };
    SidebarReviewItem {
        primary: item.navigation_primary_title(),
        secondary: item.navigation_secondary_title(),
        tooltip: item.navigation_tooltip(),
        badge: None,
        status: Some(status.to_string()),
    }
}

fn format_workspace_status(review: &ReviewSessionAppState) -> String {
    let change_count = review.diff.change_count();
    let change_part = match (review.diff.change_position(), change_count) {
        (_, 0) => "0 changes".to_string(),
        (Some(position), total) => format!("Change {position} of {total}"),
        (None, 1) => "1 change".to_string(),
        (None, total) => format!("{total} changes"),
    };
    let reviewed_part = review
        .session
        .as_ref()
        .and_then(|session| session.last_reviewed_at_utc)
        .map_or_else(
            || {
                resource(Resources::en_us(), "Review.NotMarkedReviewed")
                    .unwrap_or("Not marked reviewed")
                    .to_string()
            },
            |reviewed_at| {
                let local = reviewed_at.with_timezone(&chrono::Local);
                format!("Last reviewed {}", local.format("%Y-%m-%d %H:%M"))
            },
        );
    format!("{change_part} · {reviewed_part}")
}

fn format_selected_line_description(review: &ReviewSessionAppState) -> String {
    let Some(line) = review.selected_diff_line() else {
        return String::new();
    };
    let line_number = line.display_line_number();
    if line_number.is_empty() {
        return String::new();
    }
    let path = if line.line.file_path.is_empty() {
        review.selected_file_path.as_str()
    } else {
        line.line.file_path.as_str()
    };
    if review.selected_code_fragment.trim().is_empty() {
        format!("{path}:{line_number}")
    } else {
        format!(
            "{path}:{line_number} - selected: {}",
            review.selected_code_fragment.trim()
        )
    }
}

fn format_pending_summary(pending_count: usize) -> String {
    let resources = Resources::en_us();
    match pending_count {
        0 => resource(resources, "Review.PendingNone")
            .unwrap_or("No comments waiting to publish.")
            .to_string(),
        1 => resource(resources, "Review.PendingOne")
            .unwrap_or("1 comment waiting to publish.")
            .to_string(),
        count => format!("{count} comments waiting to publish."),
    }
}

fn format_publish_button_label(pending_count: usize) -> String {
    let resources = Resources::en_us();
    match pending_count {
        0 => resource(resources, "Review.NoCommentsToSubmit")
            .unwrap_or("No comments to submit")
            .to_string(),
        1 => resource(resources, "Review.SubmitOneComment")
            .unwrap_or("Submit 1 comment")
            .to_string(),
        count => format!("Submit {count} comments"),
    }
}

fn provider_label(reference: &PullRequestReference) -> &'static str {
    match reference.provider {
        prs_core::ReviewProvider::GitHub => "GitHub",
        prs_core::ReviewProvider::AzureDevOps => "Azure DevOps",
    }
}

fn tree_item(node: &FileTreeNodeState) -> TreeItem {
    let item = TreeItem::new(node.name.clone())
        .expanded(node.is_expanded)
        .children(node.children.iter().map(tree_item));
    if node.is_file() {
        item.detail(format!(
            "{}{}",
            node.status_code(),
            if node.is_viewed { " · viewed" } else { "" }
        ))
    } else {
        item
    }
}

fn file_node_at_path<'a>(
    nodes: &'a [FileTreeNodeState],
    path: &[usize],
) -> Option<&'a FileTreeNodeState> {
    let (&head, tail) = path.split_first()?;
    let node = nodes.get(head)?;
    if tail.is_empty() {
        Some(node)
    } else {
        file_node_at_path(&node.children, tail)
    }
}

fn file_node_at_path_mut<'a>(
    nodes: &'a mut [FileTreeNodeState],
    path: &[usize],
) -> Option<&'a mut FileTreeNodeState> {
    let (&head, tail) = path.split_first()?;
    let node = nodes.get_mut(head)?;
    if tail.is_empty() {
        Some(node)
    } else {
        file_node_at_path_mut(&mut node.children, tail)
    }
}

fn file_node_path(nodes: &[FileTreeNodeState], wanted: &str) -> Option<Vec<usize>> {
    fn find(nodes: &[FileTreeNodeState], wanted: &str, current: &mut Vec<usize>) -> bool {
        for (index, node) in nodes.iter().enumerate() {
            current.push(index);
            if node.matches_path(wanted) {
                return true;
            }
            if find(&node.children, wanted, current) {
                return true;
            }
            current.pop();
        }
        false
    }
    let mut current = Vec::new();
    find(nodes, wanted, &mut current).then_some(current)
}

fn build_diff_row(ui: &Ui, weak: Weak<AppController>, row_index: usize) -> Result<ListRow> {
    let Some(app) = weak.upgrade() else {
        return empty_diff_row(ui);
    };
    let row = {
        let state = app.state.borrow();
        state
            .active_review()
            .and_then(|review| review.diff.rows.get(row_index))
            .cloned()
    };
    let Some(row) = row else {
        return empty_diff_row(ui);
    };
    let (root, source_index) = if let Some(inline) = row.inline.as_ref() {
        (
            build_diff_line(ui, weak.clone(), inline)?,
            Some(inline.source_index),
        )
    } else if let Some(hunk) = row.hunk.as_ref() {
        (build_diff_line(ui, weak, hunk)?, None)
    } else {
        let left = row.left.as_ref();
        let right = row.right.as_ref();
        (
            build_side_by_side_diff_row(ui, weak, left, right)?,
            right.or(left).map(|line| line.source_index),
        )
    };
    let host = host_diff_element(ui, &root)?;
    let list_row = ListRow::new(&host).selectable(source_index.is_some());
    if let Some(source_index) = source_index {
        let weak = app.weak_self();
        list_row.on_activate(move || {
            if let Some(app) = weak.upgrade() {
                app.activate_diff_line(source_index);
            }
        });
    }
    Ok(list_row)
}

fn empty_diff_row(ui: &Ui) -> Result<ListRow> {
    let placeholder = ui.stack(naui::Orientation::Vertical)?;
    Ok(ListRow::new(&placeholder).selectable(false))
}

fn host_diff_element(ui: &Ui, root: &FrameworkElement) -> Result<naui::NativeHost> {
    native::normalize_diff_row_container(root);
    let host = ui.native_host()?;
    host.set_sizing(Sizing::fill_width());
    let element = raw::ui_element(root)?;
    if !host.backend().set_hosted_element(Some(&element)) {
        return Err(naui::Error::new(
            "host a raw WinUI diff row",
            "WinUI refused the diff row element",
        ));
    }
    Ok(host)
}

fn build_side_by_side_diff_row(
    ui: &Ui,
    weak: Weak<AppController>,
    left: Option<&DiffLineState>,
    right: Option<&DiffLineState>,
) -> Result<FrameworkElement> {
    let grid = raw::load::<XamlGrid>(SIDE_BY_SIDE_ROW_XAML)?;
    let root = grid
        .cast::<FrameworkElement>()
        .map_err(|error| naui::Error::new("get a side-by-side diff row", error.to_string()))?;
    let left_host = raw::element::<XamlGrid>(&root, "LeftHost")?;
    let right_host = raw::element::<XamlGrid>(&root, "RightHost")?;
    raw::set_visible_control(
        &raw::element::<FrameworkElement>(&root, "LeftEmpty")?,
        left.is_none(),
    );
    raw::set_visible_control(
        &raw::element::<FrameworkElement>(&root, "RightEmpty")?,
        right.is_none(),
    );
    if let Some(line) = left {
        append_xaml_child(
            &left_host,
            &build_diff_line(ui, weak.clone(), line)?,
            "host the left diff line",
        )?;
    }
    if let Some(line) = right {
        append_xaml_child(
            &right_host,
            &build_diff_line(ui, weak, line)?,
            "host the right diff line",
        )?;
    }
    Ok(root)
}

fn append_xaml_child(
    host: &XamlGrid,
    child: &FrameworkElement,
    context: &'static str,
) -> Result<()> {
    host.Children()
        .map_err(|error| naui::Error::new(context, error.to_string()))?
        .Append(&raw::ui_element(child)?)
        .map_err(|error| naui::Error::new(context, error.to_string()))
}

fn append_xaml_panel_child(panel: &XamlStackPanel, child: &FrameworkElement) -> Result<()> {
    append_ui_panel_child(panel, raw::ui_element(child)?)
}

fn append_ui_panel_child(panel: &XamlStackPanel, child: UIElement) -> Result<()> {
    panel
        .Children()
        .map_err(|error| naui::Error::new("get raw panel children", error.to_string()))?
        .Append(&child)
        .map_err(|error| naui::Error::new("append a raw panel child", error.to_string()))
}

fn set_xaml_text(root: &FrameworkElement, name: &str, text: &str) -> Result<()> {
    raw::element::<TextBlock>(root, name)?
        .SetText(&HSTRING::from(text))
        .map_err(|error| {
            naui::Error::new("set a raw WinUI text value", format!("'{name}': {error}"))
        })
}

fn set_xaml_visible(root: &FrameworkElement, name: &str, visible: bool) -> Result<()> {
    raw::set_visible_control(&raw::element::<FrameworkElement>(root, name)?, visible);
    Ok(())
}

fn set_comment_expansion(root: &FrameworkElement, expanded: bool) -> Result<()> {
    set_xaml_visible(root, "ExpandedContent", expanded)?;
    raw::element::<FontIcon>(root, "ExpansionGlyph")?
        .SetGlyph(&HSTRING::from(if expanded {
            "\u{E70D}"
        } else {
            "\u{E70E}"
        }))
        .map_err(|error| naui::Error::new("set a comment expansion glyph", error.to_string()))
}

fn build_diff_line(
    ui: &Ui,
    weak: Weak<AppController>,
    line: &DiffLineState,
) -> Result<FrameworkElement> {
    let grid = raw::load::<XamlGrid>(DIFF_LINE_XAML)?;
    let root = grid
        .cast::<FrameworkElement>()
        .map_err(|error| naui::Error::new("get a raw diff line", error.to_string()))?;

    for (name, visible) in [
        ("AddedBackground", line.is_added()),
        ("DeletedBackground", line.is_deleted()),
        ("HunkBackground", line.is_hunk()),
        ("AddedStroke", line.is_added()),
        ("DeletedStroke", line.is_deleted()),
        ("SelectedStroke", line.is_selected),
    ] {
        raw::set_visible_control(&raw::element::<FrameworkElement>(&root, name)?, visible);
    }

    let old_line = raw::element::<TextBlock>(&root, "OldLineNumber")?;
    old_line
        .SetText(&HSTRING::from(line.old_line_number()))
        .map_err(|error| naui::Error::new("set an old diff line number", error.to_string()))?;
    raw::set_visible_control(&old_line, line.show_old_line_number());
    let new_line = raw::element::<TextBlock>(&root, "NewLineNumber")?;
    new_line
        .SetText(&HSTRING::from(line.new_line_number()))
        .map_err(|error| naui::Error::new("set a new diff line number", error.to_string()))?;
    raw::set_visible_control(&new_line, line.show_new_line_number());
    raw::element::<ColumnDefinition>(&root, "OldLineNumberColumn")?
        .SetWidth(GridLength {
            Value: if line.show_old_line_number() {
                36.0
            } else {
                0.0
            },
            GridUnitType: GridUnitType::Pixel,
        })
        .map_err(|error| {
            naui::Error::new("set the old diff number column width", error.to_string())
        })?;
    raw::element::<ColumnDefinition>(&root, "NewLineNumberColumn")?
        .SetWidth(GridLength {
            Value: if line.show_new_line_number() {
                36.0
            } else {
                0.0
            },
            GridUnitType: GridUnitType::Pixel,
        })
        .map_err(|error| {
            naui::Error::new("set the new diff number column width", error.to_string())
        })?;

    let added_marker = raw::element::<TextBlock>(&root, "AddedMarker")?;
    added_marker
        .SetText(&HSTRING::from(line.marker()))
        .map_err(|error| naui::Error::new("set an added diff marker", error.to_string()))?;
    raw::set_visible_control(&added_marker, line.is_added());
    let deleted_marker = raw::element::<TextBlock>(&root, "DeletedMarker")?;
    deleted_marker
        .SetText(&HSTRING::from(line.marker()))
        .map_err(|error| naui::Error::new("set a deleted diff marker", error.to_string()))?;
    raw::set_visible_control(&deleted_marker, line.is_deleted());

    let code_text = raw::element::<TextBlock>(&root, "CodeText")?;
    let code_label = Label::from_native(code_text.clone());
    let mut rich = RichText::new();
    for run in line.display_runs() {
        let color = match run.kind {
            DiffHighlightKind::None => TextColor::Default,
            DiffHighlightKind::Search => TextColor::Accent,
            DiffHighlightKind::Added => TextColor::Success,
            DiffHighlightKind::Deleted => TextColor::Danger,
        };
        rich = if color == TextColor::Default {
            rich.text(run.text)
        } else {
            rich.run(TextRun::new(run.text).bold().color(color))
        };
    }
    code_label.set_rich_text(&rich);
    raw::set_visible_control(&code_text, !line.can_open_focused_comment());
    observe_code_selection(&weak, line, &code_text);

    let focused_code = raw::element::<TextBlock>(&root, "FocusedCodeText")?;
    focused_code
        .SetText(&HSTRING::from(line.code_text()))
        .map_err(|error| naui::Error::new("set focused diff code", error.to_string()))?;
    let focused_button = Button::from_button(
        ui.tasks(),
        raw::element::<XamlButton>(&root, "FocusedCodeButton")?,
    );
    focused_button.set_visible(line.can_open_focused_comment());
    focused_button.on_click({
        let weak = weak.clone();
        let source_index = line.source_index;
        move || {
            if let Some(app) = weak.upgrade() {
                app.activate_diff_line(source_index);
                app.show_focused_comment();
            }
        }
    });

    let fold_button =
        Button::from_button(ui.tasks(), raw::element::<XamlButton>(&root, "FoldButton")?);
    fold_button.set_accessible_label(resource(Resources::en_us(), "Review.FoldFunction")?);
    fold_button.set_visible(line.has_fold());
    if let Ok(glyph) = raw::element::<FontIcon>(&root, "FoldGlyph") {
        let _ = glyph.SetGlyph(&HSTRING::from(if line.is_fold_collapsed {
            "\u{E76C}"
        } else {
            "\u{E70D}"
        }));
    }
    fold_button.on_click({
        let weak = weak.clone();
        let source_index = line.source_index;
        move || {
            if let Some(app) = weak.upgrade() {
                app.toggle_fold(source_index);
            }
        }
    });

    let comment_button = Button::from_button(
        ui.tasks(),
        raw::element::<XamlButton>(&root, "CommentGutterButton")?,
    );
    comment_button.set_accessible_label(resource(Resources::en_us(), "Review.AddComment")?);
    comment_button.set_visible(line.show_normal_comment_gutter());
    comment_button.on_click({
        let weak = weak.clone();
        let source_index = line.source_index;
        move || {
            if let Some(app) = weak.upgrade() {
                app.activate_diff_line(source_index);
                app.show_focused_comment();
            }
        }
    });

    raw::set_visible_control(
        &raw::element::<FrameworkElement>(&root, "FoldSummaryPanel")?,
        line.is_fold_collapsed,
    );
    raw::element::<TextBlock>(&root, "FoldSummaryText")?
        .SetText(&HSTRING::from(format!(
            "{} lines collapsed",
            line.fold_line_count()
        )))
        .map_err(|error| naui::Error::new("set a fold summary", error.to_string()))?;
    populate_inline_comments(&weak, &root, line)?;
    Ok(root)
}

fn observe_code_selection(weak: &Weak<AppController>, line: &DiffLineState, native: &TextBlock) {
    let Some(selection_sender) = weak.upgrade().map(|app| app.selection_sender.clone()) else {
        return;
    };
    let source_index = line.source_index;
    let handler = RoutedEventHandler::new(move |sender, _| {
        if let Some(text_block) = sender
            .as_ref()
            .and_then(|value| value.cast::<TextBlock>().ok())
            && let Ok(selected) = text_block.SelectedText()
        {
            let _ = selection_sender.send((source_index, selected.to_string()));
        }
        Ok(())
    });
    if let Err(error) = native.SelectionChanged(&handler) {
        eprintln!("prs-app: source selection observation is unavailable: {error}");
    }
}

fn populate_inline_comments(
    weak: &Weak<AppController>,
    root: &FrameworkElement,
    line: &DiffLineState,
) -> Result<()> {
    let panel = raw::element::<XamlStackPanel>(root, "InlineComments")?;
    raw::set_visible_control(&panel, line.has_comments());
    let Some(app) = weak.upgrade() else {
        return Ok(());
    };
    let state = app.state.borrow();
    let Some(review) = state.active_review() else {
        return Ok(());
    };
    for comment in &review.comments {
        if line
            .inline_comment_ids
            .iter()
            .any(|id| id.eq_ignore_ascii_case(comment.id()))
        {
            let card = app.local_comment_card(comment)?;
            append_ui_panel_child(&panel, card.raw_element()?)?;
        }
    }
    for thread in &review.remote_threads {
        if line
            .inline_comment_ids
            .iter()
            .any(|id| id.eq_ignore_ascii_case(&thread.id))
        {
            let card = app.remote_thread_card(thread)?;
            append_ui_panel_child(&panel, card.raw_element()?)?;
        }
    }
    Ok(())
}

impl AppController {
    fn toggle_fold(&self, source_index: usize) {
        let changed = {
            let mut state = self.state.borrow_mut();
            state
                .active_review_mut()
                .is_some_and(|review| review.diff.toggle_fold(source_index))
        };
        if changed {
            self.render_diff();
        }
    }

    fn apply_code_selection(&self, source_index: usize, selected: String) {
        let mut state = self.state.borrow_mut();
        let Some(review) = state.active_review_mut() else {
            return;
        };
        let Some(line) = review.diff.lines.get(source_index) else {
            return;
        };
        let byte_offset = line
            .line
            .text
            .find(&selected)
            .filter(|_| !selected.is_empty())
            .unwrap_or(0);
        let start = line.line.text[..byte_offset].encode_utf16().count();
        let length = selected.encode_utf16().count();
        let _ = review.select_code_range(source_index, start, length);
        self.apply_selected_line_ui(review);
    }

    fn activate_diff_line(&self, source_index: usize) {
        let active = self
            .state
            .borrow_mut()
            .active_review_mut()
            .is_some_and(|review| review.select_diff_line(source_index));
        if active {
            self.render_comments();
            self.render_diff();
        }
    }
}

fn draw_diff_markers(painter: &mut Painter, markers: &[crate::state::DiffChangeMarkerState]) {
    let width = painter.width();
    let height = painter.height();
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    for marker in markers {
        let color = if marker.is_comment {
            TextColor::Accent
        } else if marker.is_addition {
            TextColor::Success
        } else {
            TextColor::Danger
        };
        let y = marker.relative_position.clamp(0.0, 1.0) * height;
        painter.fill_rect(
            Rect::new(4.0, (y - 2.0).max(0.0), (width - 8.0).max(4.0), 4.0),
            color,
        );
    }
}

fn draw_activity_chart(
    painter: &mut Painter,
    days: &[crate::state::ReviewActivityDayState],
    maximum: i32,
) {
    let width = painter.width();
    let height = painter.height();
    if width <= 0.0 || height <= 0.0 || days.is_empty() {
        return;
    }
    let left_gutter = 28.0;
    let top_margin = 6.0;
    let bottom = (height - 20.0).max(top_margin + 1.0);
    let chart_height = (bottom - top_margin).max(1.0);
    let plot_width = (width - left_gutter).max(1.0);
    let day_width = plot_width / days.len() as f64;
    let bar_width = (day_width / 4.5).clamp(1.5, 8.0);
    let safe_max = maximum.max(1);
    let midpoint = (safe_max + 1) / 2;

    // Subtle horizontal top, midpoint, and baseline gridlines with Y-axis labels.
    let mid_y = top_margin + chart_height * 0.5;
    painter.line(
        Point::new(left_gutter, top_margin),
        Point::new(width, top_margin),
        TextColor::Secondary,
        0.5,
    );
    painter.line(
        Point::new(left_gutter, mid_y),
        Point::new(width, mid_y),
        TextColor::Secondary,
        0.5,
    );
    painter.line(
        Point::new(left_gutter, bottom),
        Point::new(width, bottom),
        TextColor::Secondary,
        1.0,
    );
    painter.text(
        &safe_max.to_string(),
        Point::new(2.0, top_margin - 4.0),
        10.0,
        TextColor::Secondary,
    );
    if midpoint < safe_max {
        painter.text(
            &midpoint.to_string(),
            Point::new(2.0, mid_y - 6.0),
            10.0,
            TextColor::Secondary,
        );
    }
    painter.text(
        "0",
        Point::new(2.0, bottom - 10.0),
        10.0,
        TextColor::Secondary,
    );

    for (index, day) in days.iter().enumerate() {
        let center = left_gutter + index as f64 * day_width + day_width / 2.0;
        let values = [
            (day.pull_request_count, TextColor::Accent, -1.2),
            (day.comment_count, TextColor::Success, 0.0),
            (day.review_count, TextColor::Warning, 1.2),
        ];
        for (value, color, offset) in values {
            if value <= 0 {
                continue;
            }
            let bar_height = chart_height * f64::from(value) / f64::from(safe_max);
            painter.fill_rect(
                Rect::new(
                    center + bar_width * offset - bar_width * 0.5,
                    bottom - bar_height,
                    bar_width,
                    bar_height,
                ),
                color,
            );
        }
        if days.len() <= 31 {
            let label = day.day_label();
            let x_offset = if label.len() > 1 { 5.0 } else { 3.0 };
            painter.text(
                &label,
                Point::new((center - x_offset).max(left_gutter), bottom + 4.0),
                10.0,
                TextColor::Secondary,
            );
        }
    }
}

fn build_markdown_file_row(
    tasks: Tasks,
    weak: Weak<AppController>,
    block: &MarkdownBlock,
    comment_name: &str,
    comment_tooltip: &str,
) -> Result<UIElement> {
    const ROW_XAML: &str = r#"<Grid
        xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        Padding="4,4"
        ColumnSpacing="12">
        <Grid.ColumnDefinitions>
            <ColumnDefinition Width="40" />
            <ColumnDefinition Width="*" />
            <ColumnDefinition Width="Auto" />
        </Grid.ColumnDefinitions>
        <TextBlock
            x:Name="LineLabel"
            HorizontalAlignment="Right"
            VerticalAlignment="Top"
            Margin="0,2,0,0"
            Foreground="{ThemeResource TextFillColorTertiaryBrush}"
            Style="{StaticResource CaptionTextBlockStyle}"
            Typography.NumeralAlignment="Tabular" />
        <Grid x:Name="BodyHost" Grid.Column="1" />
        <Button
            x:Name="BlockCommentButton"
            Grid.Column="2"
            Padding="4"
            VerticalAlignment="Top"
            Style="{StaticResource SubtleButtonStyle}">
            <FontIcon FontSize="12" Glyph="&#xE8BD;" />
        </Button>
    </Grid>"#;

    let row = raw::load::<XamlGrid>(ROW_XAML)?;
    let root = row
        .cast::<FrameworkElement>()
        .map_err(|error| naui::Error::new("create a Markdown row", error.to_string()))?;
    let line_label = if block.source_line_number > 0 {
        block.source_line_number.to_string()
    } else {
        String::new()
    };
    set_xaml_text(&root, "LineLabel", &line_label)?;
    let body_host = raw::element::<XamlGrid>(&root, "BodyHost")?;
    let body = build_markdown_body_element(block)?;
    body_host
        .Children()
        .map_err(|error| naui::Error::new("get Markdown body host children", error.to_string()))?
        .Append(&body)
        .map_err(|error| naui::Error::new("append Markdown body", error.to_string()))?;

    let comment_btn = button(tasks, &root, "BlockCommentButton")?;
    comment_btn.set_accessible_label(comment_name);
    comment_btn.set_tooltip(comment_tooltip);
    let can_comment =
        block.source_line_number > 0 && block.kind != MarkdownBlockKind::HorizontalRule;
    comment_btn.set_visible(can_comment);
    let source_line = block.source_line_number;
    comment_btn.on_click(move || {
        if let Some(app) = weak.upgrade() {
            app.show_comment_for_source_line(source_line);
        }
    });

    raw::ui_element(&root)
}

fn build_markdown_body_element(block: &MarkdownBlock) -> Result<UIElement> {
    let xaml = match block.kind {
        MarkdownBlockKind::Heading1 => {
            r#"<TextBlock xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" x:Name="Body" Style="{StaticResource TitleTextBlockStyle}" IsTextSelectionEnabled="True" TextWrapping="Wrap" />"#
        }
        MarkdownBlockKind::Heading2 => {
            r#"<TextBlock xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" x:Name="Body" Style="{StaticResource SubtitleTextBlockStyle}" IsTextSelectionEnabled="True" TextWrapping="Wrap" />"#
        }
        MarkdownBlockKind::Heading3 | MarkdownBlockKind::Heading4 => {
            r#"<TextBlock xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" x:Name="Body" Style="{StaticResource BodyStrongTextBlockStyle}" IsTextSelectionEnabled="True" TextWrapping="Wrap" />"#
        }
        MarkdownBlockKind::Heading5 | MarkdownBlockKind::Heading6 => {
            r#"<TextBlock xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" x:Name="Body" Foreground="{ThemeResource TextFillColorSecondaryBrush}" Style="{StaticResource CaptionTextBlockStyle}" IsTextSelectionEnabled="True" TextWrapping="Wrap" />"#
        }
        MarkdownBlockKind::Bullet | MarkdownBlockKind::Numbered => {
            r#"<Grid xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" ColumnSpacing="8">
                <Grid.ColumnDefinitions><ColumnDefinition Width="Auto" /><ColumnDefinition Width="*" /></Grid.ColumnDefinitions>
                <TextBlock x:Name="Marker" Foreground="{ThemeResource TextFillColorSecondaryBrush}" />
                <TextBlock x:Name="Body" Grid.Column="1" IsTextSelectionEnabled="True" TextWrapping="Wrap" />
            </Grid>"#
        }
        MarkdownBlockKind::TaskChecked => {
            r#"<Grid xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" ColumnSpacing="8">
                <Grid.ColumnDefinitions><ColumnDefinition Width="Auto" /><ColumnDefinition Width="*" /></Grid.ColumnDefinitions>
                <FontIcon FontSize="14" Glyph="&#xE73E;" Foreground="{ThemeResource SystemFillColorSuccessBrush}" VerticalAlignment="Top" Margin="0,2,0,0" />
                <TextBlock x:Name="Body" Grid.Column="1" IsTextSelectionEnabled="True" TextWrapping="Wrap" />
            </Grid>"#
        }
        MarkdownBlockKind::TaskUnchecked => {
            r#"<Grid xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" ColumnSpacing="8">
                <Grid.ColumnDefinitions><ColumnDefinition Width="Auto" /><ColumnDefinition Width="*" /></Grid.ColumnDefinitions>
                <FontIcon FontSize="14" Glyph="&#xE739;" Foreground="{ThemeResource TextFillColorSecondaryBrush}" VerticalAlignment="Top" Margin="0,2,0,0" />
                <TextBlock x:Name="Body" Grid.Column="1" IsTextSelectionEnabled="True" TextWrapping="Wrap" />
            </Grid>"#
        }
        MarkdownBlockKind::Quote => {
            r#"<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" Padding="12,4" BorderBrush="{ThemeResource AccentFillColorDefaultBrush}" BorderThickness="3,0,0,0">
                <TextBlock x:Name="Body" FontStyle="Italic" Foreground="{ThemeResource TextFillColorSecondaryBrush}" IsTextSelectionEnabled="True" TextWrapping="Wrap" />
            </Border>"#
        }
        MarkdownBlockKind::Code => {
            r#"<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" Padding="12" Background="{ThemeResource ControlFillColorSecondaryBrush}" CornerRadius="{ThemeResource ControlCornerRadius}">
                <TextBlock x:Name="Body" FontFamily="Cascadia Mono" FontSize="13" IsTextSelectionEnabled="True" TextWrapping="Wrap" />
            </Border>"#
        }
        MarkdownBlockKind::Table => {
            r#"<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" Padding="12" BorderBrush="{ThemeResource CardStrokeColorDefaultBrush}" BorderThickness="1" CornerRadius="{ThemeResource ControlCornerRadius}">
                <TextBlock x:Name="Body" FontFamily="Cascadia Mono" FontSize="13" IsTextSelectionEnabled="True" TextWrapping="NoWrap" />
            </Border>"#
        }
        MarkdownBlockKind::HorizontalRule => {
            return raw::ui_element(&raw::load::<FrameworkElement>(
                r#"<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" Height="1" Margin="0,4" Background="{ThemeResource DividerStrokeColorDefaultBrush}" />"#,
            )?);
        }
        MarkdownBlockKind::Paragraph => {
            r#"<TextBlock xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" x:Name="Body" IsTextSelectionEnabled="True" TextWrapping="Wrap" />"#
        }
    };

    let root = raw::load::<FrameworkElement>(xaml)?;
    if matches!(
        block.kind,
        MarkdownBlockKind::Bullet | MarkdownBlockKind::Numbered
    ) {
        let marker = block
            .list_marker
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("•");
        set_xaml_text(&root, "Marker", marker)?;
    }
    if let Ok(body_text) = raw::element::<TextBlock>(&root, "Body") {
        let body_label = Label::from_native(body_text);
        if matches!(
            block.kind,
            MarkdownBlockKind::Code | MarkdownBlockKind::Table
        ) {
            body_label.set_text(&block.text);
        } else {
            let mut rich = RichText::new();
            if let Some(runs) = block.inline_runs.as_deref() {
                for run in runs {
                    match run.kind {
                        MarkdownInlineKind::Text => rich = rich.text(run.text.clone()),
                        MarkdownInlineKind::Bold => rich = rich.bold(run.text.clone()),
                        MarkdownInlineKind::Code => {
                            rich = rich.colored(run.text.clone(), TextColor::Accent);
                        }
                        MarkdownInlineKind::Link => {
                            rich = rich.colored(run.text.clone(), TextColor::Accent);
                        }
                    }
                }
            } else {
                rich = rich.text(block.text.clone());
            }
            body_label.set_rich_text(&rich);
        }
    }
    raw::ui_element(&root)
}
