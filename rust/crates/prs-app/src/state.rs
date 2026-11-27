use chrono::{Datelike, Local, NaiveDate};
use prs_core::{
    ChangedFile, ChangedFileStatus, CodeFoldRegion, DiffBlock, DiffLine, DiffLineKind,
    MarkdownBlock, PullRequestDetails, PullRequestIteration, RemoteReviewComment,
    ReviewActivityDay, ReviewActivityKind, ReviewActivityRecord, ReviewComment, ReviewCommentSide,
    ReviewCommentStatus, ReviewProvider, ReviewSessionState, SymbolCandidate, SymbolLocation,
    build_folds, build_side_by_side, extract_symbol_candidates, markdown,
};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;

fn normalized_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn paths_equal(left: &str, right: &str) -> bool {
    normalized_path(left).eq_ignore_ascii_case(&normalized_path(right))
}

fn text_equal(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn reversed_order(left: i64, right: i64) -> Ordering {
    right.cmp(&left)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewNavigationStatus {
    #[default]
    Open,
    Archived,
    Closed,
    Merged,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewNavigationItemState {
    pub session_key: String,
    pub provider: ReviewProvider,
    pub host: String,
    pub repository: String,
    pub title: String,
    pub pull_request_number: u32,
    pub web_url: String,
    pub author: String,
    pub status: ReviewNavigationStatus,
    pub is_archived: bool,
    last_opened_sort_key: i64,
    updated_sort_key: i64,
}

impl ReviewNavigationItemState {
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.status == ReviewNavigationStatus::Open
    }

    #[must_use]
    pub fn is_reviewed(&self) -> bool {
        !self.is_open()
    }

    #[must_use]
    pub fn navigation_primary_title(&self) -> String {
        format!("#{} · {}", self.pull_request_number, self.title)
    }

    #[must_use]
    pub fn navigation_secondary_title(&self) -> String {
        format!("{} · {}", self.host, self.repository)
    }

    #[must_use]
    pub fn navigation_tooltip(&self) -> String {
        format!(
            "{}\n{}",
            self.navigation_primary_title(),
            self.navigation_secondary_title()
        )
    }

    #[must_use]
    pub fn provider_label(&self) -> &'static str {
        match self.provider {
            ReviewProvider::GitHub => "GitHub",
            ReviewProvider::AzureDevOps => "Azure DevOps",
        }
    }
}

impl From<&ReviewSessionState> for ReviewNavigationItemState {
    fn from(state: &ReviewSessionState) -> Self {
        let status = navigation_status(
            &state.details,
            state.is_archived,
            state.completed_at_utc.is_some(),
        );
        Self {
            session_key: state.details.reference.session_key.clone(),
            provider: state.details.reference.provider,
            host: state.details.reference.host.clone(),
            repository: state.details.reference.repository.clone(),
            title: state.details.title.clone(),
            pull_request_number: state.details.reference.number,
            web_url: state.details.reference.web_url.clone(),
            author: state.details.author.clone(),
            status,
            is_archived: state.is_archived,
            last_opened_sort_key: state.last_opened_utc.timestamp_millis(),
            updated_sort_key: state.details.updated_at_utc.map_or_else(
                || state.last_opened_utc.timestamp_millis(),
                |updated| updated.timestamp_millis(),
            ),
        }
    }
}

#[must_use]
pub fn navigation_status(
    details: &PullRequestDetails,
    is_archived: bool,
    is_completed: bool,
) -> ReviewNavigationStatus {
    if is_completed {
        ReviewNavigationStatus::Completed
    } else if details.is_merged {
        ReviewNavigationStatus::Merged
    } else if is_provider_terminal(details) {
        ReviewNavigationStatus::Closed
    } else if is_archived {
        ReviewNavigationStatus::Archived
    } else {
        ReviewNavigationStatus::Open
    }
}

#[must_use]
pub fn is_provider_terminal(details: &PullRequestDetails) -> bool {
    details.is_closed
        || details.state.eq_ignore_ascii_case("closed")
        || details.state.eq_ignore_ascii_case("completed")
        || details.state.eq_ignore_ascii_case("abandoned")
}

#[derive(Clone, PartialEq)]
pub struct MainAppState {
    pub pull_request_url: String,
    pub local_repository_path: String,
    pub is_opening: bool,
    pub error_message: String,
    pub status_message: String,
    pub active_review_key: Option<String>,
    pub active_reviews: Vec<ReviewNavigationItemState>,
    pub reviewed_reviews: Vec<ReviewNavigationItemState>,
    pub all_reviews: Vec<ReviewNavigationItemState>,
    pub is_code_view_expanded: bool,
    pub is_open_review_panel_visible: bool,
    pub is_activity_view_visible: bool,
    pub changed_files_pane_width: f64,
    pub review_inspector_pane_width: f64,
    pub authentication_required: bool,
    pub authentication_message: String,
}

impl Default for MainAppState {
    fn default() -> Self {
        Self {
            pull_request_url: String::new(),
            local_repository_path: String::new(),
            is_opening: false,
            error_message: String::new(),
            status_message: String::new(),
            active_review_key: None,
            active_reviews: Vec::new(),
            reviewed_reviews: Vec::new(),
            all_reviews: Vec::new(),
            is_code_view_expanded: false,
            is_open_review_panel_visible: true,
            is_activity_view_visible: false,
            changed_files_pane_width: 240.0,
            review_inspector_pane_width: 320.0,
            authentication_required: false,
            authentication_message: String::new(),
        }
    }
}

impl fmt::Debug for MainAppState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MainAppState")
            .field("is_opening", &self.is_opening)
            .field("active_review_key", &self.active_review_key)
            .field("active_review_count", &self.active_reviews.len())
            .field("reviewed_review_count", &self.reviewed_reviews.len())
            .field("is_code_view_expanded", &self.is_code_view_expanded)
            .field(
                "is_open_review_panel_visible",
                &self.is_open_review_panel_visible,
            )
            .field("is_activity_view_visible", &self.is_activity_view_visible)
            .finish_non_exhaustive()
    }
}

impl MainAppState {
    #[must_use]
    pub fn from_review_states<'a>(
        states: impl IntoIterator<Item = &'a ReviewSessionState>,
    ) -> Self {
        let mut state = Self::default();
        for review in states {
            state.track_review(review);
        }
        state.select_initial_review();
        state
    }

    #[must_use]
    pub fn can_open_review(&self) -> bool {
        !self.is_opening && !self.pull_request_url.trim().is_empty()
    }

    #[must_use]
    pub fn can_authenticate(&self) -> bool {
        self.authentication_required && !self.is_opening
    }

    #[must_use]
    pub fn has_error(&self) -> bool {
        !self.error_message.trim().is_empty()
    }

    #[must_use]
    pub fn has_review_alert(&self) -> bool {
        self.authentication_required || self.has_error()
    }

    #[must_use]
    pub fn is_review_view_visible(&self) -> bool {
        !self.is_activity_view_visible
    }

    #[must_use]
    pub fn is_normal_review_layout(&self) -> bool {
        !self.is_code_view_expanded
    }

    #[must_use]
    pub fn is_review_panel_visible(&self) -> bool {
        self.is_open_review_panel_visible && self.is_normal_review_layout()
    }

    #[must_use]
    pub fn is_review_alert_visible(&self) -> bool {
        self.has_review_alert() && self.is_normal_review_layout()
    }

    #[must_use]
    pub fn selected_review(&self) -> Option<&ReviewNavigationItemState> {
        let selected = self.active_review_key.as_deref()?;
        self.all_reviews
            .iter()
            .find(|review| text_equal(&review.session_key, selected))
    }

    #[must_use]
    pub fn selected_open_review(&self) -> Option<&ReviewNavigationItemState> {
        if self.is_activity_view_visible {
            return None;
        }
        let selected = self.selected_review()?;
        self.active_reviews
            .iter()
            .find(|review| text_equal(&review.session_key, &selected.session_key))
    }

    pub fn track_review(&mut self, review: &ReviewSessionState) -> bool {
        let item = ReviewNavigationItemState::from(review);
        if item.session_key.is_empty() {
            return false;
        }

        if let Some(existing) = self
            .all_reviews
            .iter_mut()
            .find(|existing| text_equal(&existing.session_key, &item.session_key))
        {
            *existing = item;
        } else {
            self.all_reviews.push(item);
        }
        self.reclassify_reviews();
        true
    }

    pub fn set_reviews<'a>(&mut self, states: impl IntoIterator<Item = &'a ReviewSessionState>) {
        let prior_order = self
            .all_reviews
            .iter()
            .map(|review| review.session_key.clone())
            .collect::<HashSet<_>>();
        let items = states
            .into_iter()
            .map(ReviewNavigationItemState::from)
            .collect::<Vec<_>>();
        let mut unique = Vec::with_capacity(items.len());
        for item in items {
            if !unique.iter().any(|existing: &ReviewNavigationItemState| {
                text_equal(&existing.session_key, &item.session_key)
            }) {
                unique.push(item);
            }
        }
        unique.sort_by_key(|item| !prior_order.contains(&item.session_key));
        self.all_reviews = unique;
        self.reclassify_reviews();
        if self.active_review_key.as_deref().is_some_and(|key| {
            !self
                .all_reviews
                .iter()
                .any(|review| text_equal(&review.session_key, key))
        }) {
            self.active_review_key = None;
        }
        if self.active_review_key.is_none() {
            self.select_initial_review();
        }
    }

    pub fn select_review(&mut self, session_key: &str) -> bool {
        if !self
            .all_reviews
            .iter()
            .any(|review| text_equal(&review.session_key, session_key))
        {
            return false;
        }
        self.active_review_key = Some(
            self.all_reviews
                .iter()
                .find(|review| text_equal(&review.session_key, session_key))
                .expect("the review key was just located")
                .session_key
                .clone(),
        );
        self.is_activity_view_visible = false;
        self.is_open_review_panel_visible = false;
        true
    }

    pub fn remove_review(&mut self, session_key: &str) -> bool {
        let Some(index) = self
            .all_reviews
            .iter()
            .position(|review| text_equal(&review.session_key, session_key))
        else {
            return false;
        };
        let removed = self.all_reviews.remove(index);
        self.reclassify_reviews();
        if self
            .active_review_key
            .as_deref()
            .is_some_and(|key| text_equal(key, &removed.session_key))
        {
            self.active_review_key = None;
            self.select_initial_review();
        }
        true
    }

    pub fn archive_selected_review(&mut self) -> bool {
        let Some(selected_key) = self.active_review_key.clone() else {
            return false;
        };
        let Some(review) = self
            .all_reviews
            .iter_mut()
            .find(|review| text_equal(&review.session_key, &selected_key))
        else {
            return false;
        };
        if !review.is_open() {
            return false;
        }
        review.is_archived = true;
        review.status = ReviewNavigationStatus::Archived;
        self.reclassify_reviews();
        true
    }

    pub fn close_selected_review(&mut self) -> bool {
        let was_open = self
            .active_review_key
            .as_deref()
            .and_then(|key| {
                self.all_reviews
                    .iter()
                    .find(|review| text_equal(&review.session_key, key))
            })
            .is_some_and(ReviewNavigationItemState::is_open);
        if !was_open || !self.archive_selected_review() {
            return false;
        }
        self.active_review_key = None;
        self.select_initial_review();
        true
    }

    pub fn complete_selected_review(&mut self) -> bool {
        let Some(session_key) = self.active_review_key.clone() else {
            return false;
        };
        self.remove_review(&session_key)
    }

    pub fn reopen_selected_review(&mut self) -> bool {
        let Some(selected_key) = self.active_review_key.clone() else {
            return false;
        };
        let Some(review) = self
            .all_reviews
            .iter_mut()
            .find(|review| text_equal(&review.session_key, &selected_key))
        else {
            return false;
        };
        if review.status != ReviewNavigationStatus::Archived {
            return false;
        }
        review.is_archived = false;
        review.status = ReviewNavigationStatus::Open;
        self.reclassify_reviews();
        true
    }

    pub fn show_open_review_panel(&mut self) {
        self.is_activity_view_visible = false;
        self.is_open_review_panel_visible = true;
    }

    pub fn hide_open_review_panel(&mut self) {
        if self.active_review_key.is_some() {
            self.is_open_review_panel_visible = false;
        }
    }

    pub fn show_activity(&mut self) {
        self.is_activity_view_visible = true;
    }

    pub fn hide_activity(&mut self) {
        self.is_activity_view_visible = false;
    }

    pub fn toggle_code_view(&mut self) {
        self.is_code_view_expanded = !self.is_code_view_expanded;
    }

    #[must_use]
    pub fn changed_files_pane_width_value(&self) -> f64 {
        if self.is_code_view_expanded {
            0.0
        } else {
            self.changed_files_pane_width
        }
    }

    #[must_use]
    pub fn review_inspector_pane_width_value(&self) -> f64 {
        if self.is_code_view_expanded {
            0.0
        } else {
            self.review_inspector_pane_width
        }
    }

    #[must_use]
    pub fn pane_splitter_width(&self) -> f64 {
        if self.is_code_view_expanded {
            0.0
        } else {
            12.0
        }
    }

    pub fn resize_changed_files_pane(&mut self, horizontal_change: f64) {
        self.changed_files_pane_width =
            (self.changed_files_pane_width + horizontal_change).clamp(200.0, 420.0);
    }

    pub fn resize_review_inspector_pane(&mut self, horizontal_change: f64) {
        self.review_inspector_pane_width =
            (self.review_inspector_pane_width - horizontal_change).clamp(280.0, 500.0);
    }

    pub fn set_authentication_required(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.authentication_required = !message.trim().is_empty();
        self.authentication_message = message;
    }

    pub fn clear_authentication_required(&mut self) {
        self.authentication_required = false;
        self.authentication_message.clear();
    }

    fn select_initial_review(&mut self) {
        self.active_review_key = self
            .active_reviews
            .first()
            .or_else(|| self.reviewed_reviews.first())
            .map(|review| review.session_key.clone());
        self.is_open_review_panel_visible = self.active_review_key.is_none();
    }

    fn reclassify_reviews(&mut self) {
        let mut active = self
            .all_reviews
            .iter()
            .filter(|review| review.is_open())
            .cloned()
            .collect::<Vec<_>>();
        active.sort_by(|left, right| {
            reversed_order(left.last_opened_sort_key, right.last_opened_sort_key)
        });
        let mut reviewed = self
            .all_reviews
            .iter()
            .filter(|review| review.is_reviewed())
            .cloned()
            .collect::<Vec<_>>();
        reviewed
            .sort_by(|left, right| reversed_order(left.updated_sort_key, right.updated_sort_key));
        synchronize_review_items(&mut self.active_reviews, &active);
        synchronize_review_items(&mut self.reviewed_reviews, &reviewed);
    }
}

impl From<&ReviewSessionState> for MainAppState {
    fn from(state: &ReviewSessionState) -> Self {
        Self::from_review_states(std::iter::once(state))
    }
}

impl From<&[ReviewSessionState]> for MainAppState {
    fn from(states: &[ReviewSessionState]) -> Self {
        Self::from_review_states(states)
    }
}

fn synchronize_review_items(
    target: &mut Vec<ReviewNavigationItemState>,
    desired: &[ReviewNavigationItemState],
) {
    for (index, item) in desired.iter().enumerate() {
        let existing = target
            .iter()
            .position(|candidate| text_equal(&candidate.session_key, &item.session_key));
        match existing {
            Some(existing_index) if existing_index == index => {
                target[index] = item.clone();
            }
            Some(existing_index) => {
                let existing = target.remove(existing_index);
                let replacement = ReviewNavigationItemState { ..item.clone() };
                target.insert(index, existing);
                target[index] = replacement;
            }
            None => target.insert(index, item.clone()),
        }
    }
    target.truncate(desired.len());
}

#[derive(Clone, PartialEq, Eq)]
pub struct FileTreeNodeState {
    pub name: String,
    pub path: String,
    pub is_folder: bool,
    pub changed_file: Option<ChangedFile>,
    pub children: Vec<FileTreeNodeState>,
    pub parent_path: Option<String>,
    pub is_expanded: bool,
    pub is_viewed: bool,
}

impl FileTreeNodeState {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        path: impl Into<String>,
        is_folder: bool,
        changed_file: Option<ChangedFile>,
    ) -> Self {
        Self {
            name: name.into(),
            path: normalized_path(&path.into()),
            is_folder,
            changed_file,
            children: Vec::new(),
            parent_path: None,
            is_expanded: false,
            is_viewed: false,
        }
    }

    #[must_use]
    pub const fn is_file(&self) -> bool {
        !self.is_folder
    }

    #[must_use]
    pub const fn is_unviewed(&self) -> bool {
        self.is_file() && !self.is_viewed
    }

    #[must_use]
    pub fn status_code(&self) -> &'static str {
        match self.changed_file.as_ref().map(|file| file.status) {
            Some(ChangedFileStatus::Added) => "A",
            Some(ChangedFileStatus::Deleted) => "D",
            Some(ChangedFileStatus::Renamed) => "R",
            Some(ChangedFileStatus::Copied) => "C",
            Some(ChangedFileStatus::TypeChanged) => "T",
            Some(ChangedFileStatus::Unmerged) => "U",
            _ => "M",
        }
    }

    #[must_use]
    pub fn status(&self) -> String {
        self.status_code().to_string()
    }

    #[must_use]
    pub fn is_added_status(&self) -> bool {
        self.changed_file
            .as_ref()
            .is_some_and(|file| file.status == ChangedFileStatus::Added)
    }

    #[must_use]
    pub fn is_modified_status(&self) -> bool {
        self.changed_file
            .as_ref()
            .is_some_and(|file| file.status == ChangedFileStatus::Modified)
    }

    #[must_use]
    pub fn is_deleted_status(&self) -> bool {
        self.changed_file
            .as_ref()
            .is_some_and(|file| file.status == ChangedFileStatus::Deleted)
    }

    #[must_use]
    pub fn is_other_status(&self) -> bool {
        self.is_file()
            && !self.is_added_status()
            && !self.is_modified_status()
            && !self.is_deleted_status()
    }

    #[must_use]
    pub fn accessibility_name(&self) -> String {
        if self.is_file() {
            format!(
                "{:?} file {}{}",
                self.changed_file
                    .as_ref()
                    .map_or(ChangedFileStatus::Modified, |file| file.status),
                self.path,
                if self.is_viewed { ", viewed" } else { "" }
            )
        } else {
            format!("Folder {}", self.path)
        }
    }

    #[must_use]
    pub fn matches_path(&self, path: &str) -> bool {
        paths_equal(&self.path, path)
    }

    #[must_use]
    pub fn find<'a>(nodes: &'a [Self], path: &str) -> Option<&'a FileTreeNodeState> {
        for node in nodes {
            if node.matches_path(path) {
                return Some(node);
            }
            if let Some(found) = Self::find(&node.children, path) {
                return Some(found);
            }
        }
        None
    }

    #[must_use]
    pub fn find_mut<'a>(nodes: &'a mut [Self], path: &str) -> Option<&'a mut FileTreeNodeState> {
        for node in nodes {
            if node.matches_path(path) {
                return Some(node);
            }
            if let Some(found) = Self::find_mut(&mut node.children, path) {
                return Some(found);
            }
        }
        None
    }

    #[must_use]
    pub fn find_first_file(nodes: &[Self]) -> Option<&FileTreeNodeState> {
        for node in nodes {
            if node.is_file() {
                return Some(node);
            }
            if let Some(found) = Self::find_first_file(&node.children) {
                return Some(found);
            }
        }
        None
    }

    pub fn expand_path(roots: &mut [Self], path: &str) {
        let mut parts = path.split('/').collect::<Vec<_>>();
        while !parts.is_empty() {
            parts.pop();
            if parts.is_empty() {
                break;
            }
            let ancestor = parts.join("/");
            if let Some(node) = Self::find_mut(roots, &ancestor) {
                node.is_expanded = true;
            }
        }
    }

    pub fn expand(&mut self, expanded: bool) {
        self.is_expanded = expanded;
    }

    pub fn expand_ancestors(roots: &mut [Self], path: &str) -> bool {
        let Some(node) = Self::find(roots, path) else {
            return false;
        };
        let mut parent = node.parent_path.clone();
        while let Some(parent_path) = parent {
            let Some(parent_node) = Self::find_mut(roots, &parent_path) else {
                break;
            };
            parent_node.is_expanded = true;
            parent = parent_node.parent_path.clone();
        }
        true
    }

    fn sort_children_recursively(&mut self) {
        self.children.sort_by(compare_tree_nodes);
        for child in &mut self.children {
            child.sort_children_recursively();
        }
    }

    fn compact_single_child_folders(&mut self) {
        if !self.is_folder {
            return;
        }
        while self.children.len() == 1 && self.children[0].is_folder {
            let child = self.children.remove(0);
            self.name = format!("{}/{}", self.name, child.name);
            self.path = child.path;
            self.children = child.children;
        }
        for child in &mut self.children {
            child.compact_single_child_folders();
        }
        self.repair_parent_paths();
    }

    fn repair_parent_paths(&mut self) {
        let parent_path = self.path.clone();
        for child in &mut self.children {
            child.parent_path = Some(parent_path.clone());
            child.repair_parent_paths();
        }
    }
}

impl fmt::Debug for FileTreeNodeState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileTreeNodeState")
            .field("name", &self.name)
            .field("path", &self.path)
            .field("is_folder", &self.is_folder)
            .field("is_viewed", &self.is_viewed)
            .field("children", &self.children)
            .finish()
    }
}

fn compare_tree_nodes(left: &FileTreeNodeState, right: &FileTreeNodeState) -> Ordering {
    right
        .is_folder
        .cmp(&left.is_folder)
        .then_with(|| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
        })
        .then_with(|| left.name.cmp(&right.name))
        .then_with(|| left.path.cmp(&right.path))
}

pub struct FileTreeBuilder;

impl FileTreeBuilder {
    #[must_use]
    pub fn build(
        changed_files: &[ChangedFile],
        viewed_paths: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Vec<FileTreeNodeState> {
        let viewed = viewed_paths
            .into_iter()
            .map(|path| normalized_path(path.as_ref()).to_ascii_lowercase())
            .collect::<HashSet<_>>();
        let mut files = changed_files.iter().collect::<Vec<_>>();
        files.sort_by(|left, right| {
            left.path
                .to_ascii_lowercase()
                .cmp(&right.path.to_ascii_lowercase())
                .then_with(|| left.path.cmp(&right.path))
        });
        let mut roots = Vec::<FileTreeNodeState>::new();
        for file in files {
            let path = normalized_path(&file.path);
            let segments = path
                .split('/')
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>();
            if segments.is_empty() {
                continue;
            }

            let mut siblings = &mut roots;
            let mut current_path = String::new();
            for (index, segment) in segments.iter().enumerate() {
                current_path = if current_path.is_empty() {
                    (*segment).to_string()
                } else {
                    format!("{current_path}/{segment}")
                };
                let is_file = index + 1 == segments.len();
                let child_index = siblings
                    .iter()
                    .position(|node| text_equal(&node.name, segment))
                    .unwrap_or_else(|| {
                        let mut node = FileTreeNodeState::new(
                            *segment,
                            &current_path,
                            !is_file,
                            is_file.then(|| file.clone()),
                        );
                        if is_file {
                            node.is_viewed = viewed.contains(&current_path.to_ascii_lowercase());
                        }
                        siblings.push(node);
                        siblings.len() - 1
                    });
                siblings = &mut siblings[child_index].children;
            }
        }

        for root in &mut roots {
            root.compact_single_child_folders();
        }
        roots.sort_by(compare_tree_nodes);
        for root in &mut roots {
            root.sort_children_recursively();
        }
        roots
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffHighlightKind {
    #[default]
    None,
    Search,
    Added,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffCodeTextRun {
    pub text: String,
    pub kind: DiffHighlightKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SymbolLinkState {
    pub symbol: String,
    pub display_name: String,
    pub column_index: usize,
    pub leading_offset: f64,
}

impl From<&SymbolCandidate> for SymbolLinkState {
    fn from(candidate: &SymbolCandidate) -> Self {
        Self {
            symbol: candidate.name.clone(),
            display_name: candidate.display_name.clone(),
            column_index: candidate.column_index,
            leading_offset: 8.0_f64.max((candidate.column_index as f64 * 7.8).min(720.0)),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct DiffLineState {
    pub line: DiffLine,
    pub source_index: usize,
    pub display_side: Option<ReviewCommentSide>,
    pub is_selected: bool,
    pub is_focused_comment_mode: bool,
    pub is_search_match: bool,
    pub is_current_search_match: bool,
    pub navigation_symbols: Vec<SymbolLinkState>,
    pub selection_start: usize,
    pub selection_length: usize,
    pub fold_region: Option<CodeFoldRegion>,
    pub is_fold_collapsed: bool,
    pub inline_comment_ids: Vec<String>,
    pub left_comment_ids: Vec<String>,
    pub right_comment_ids: Vec<String>,
    search_query: String,
    difference_prefix: String,
    difference_text: String,
    difference_suffix: String,
}

impl Default for DiffLineState {
    fn default() -> Self {
        Self::from_line(DiffLine::default(), 0)
    }
}

impl fmt::Debug for DiffLineState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiffLineState")
            .field("kind", &self.line.kind)
            .field("file_path", &self.line.file_path)
            .field("old_line_number", &self.line.old_line_number)
            .field("new_line_number", &self.line.new_line_number)
            .field("source_index", &self.source_index)
            .field("display_side", &self.display_side)
            .field("is_selected", &self.is_selected)
            .field("is_search_match", &self.is_search_match)
            .field("is_fold_collapsed", &self.is_fold_collapsed)
            .field("comment_count", &self.comment_count())
            .finish_non_exhaustive()
    }
}

impl DiffLineState {
    #[must_use]
    pub fn new(line: DiffLine) -> Self {
        Self::from_line(line, 0)
    }

    #[must_use]
    pub fn from_line(line: DiffLine, source_index: usize) -> Self {
        let navigation_symbols = extract_symbol_candidates(&line.text)
            .iter()
            .filter(|candidate| !candidate.is_likely_external)
            .map(SymbolLinkState::from)
            .collect();
        Self {
            line,
            source_index,
            display_side: None,
            is_selected: false,
            is_focused_comment_mode: false,
            is_search_match: false,
            is_current_search_match: false,
            navigation_symbols,
            selection_start: 0,
            selection_length: 0,
            fold_region: None,
            is_fold_collapsed: false,
            inline_comment_ids: Vec::new(),
            left_comment_ids: Vec::new(),
            right_comment_ids: Vec::new(),
            search_query: String::new(),
            difference_prefix: String::new(),
            difference_text: String::new(),
            difference_suffix: String::new(),
        }
    }

    #[must_use]
    pub fn from_ref(line: &DiffLine, source_index: usize) -> Self {
        Self::from_line(line.clone(), source_index)
    }

    #[must_use]
    pub fn old_line_number(&self) -> String {
        self.line
            .old_line_number
            .map_or_else(String::new, |line| line.to_string())
    }

    #[must_use]
    pub fn new_line_number(&self) -> String {
        self.line
            .new_line_number
            .map_or_else(String::new, |line| line.to_string())
    }

    #[must_use]
    pub fn code_text(&self) -> &str {
        &self.line.text
    }

    #[must_use]
    pub fn marker(&self) -> &'static str {
        match self.line.kind {
            DiffLineKind::Added => "+",
            DiffLineKind::Deleted => "-",
            DiffLineKind::Context | DiffLineKind::Hunk => "",
        }
    }

    #[must_use]
    pub const fn is_added(&self) -> bool {
        matches!(self.line.kind, DiffLineKind::Added)
    }

    #[must_use]
    pub const fn is_deleted(&self) -> bool {
        matches!(self.line.kind, DiffLineKind::Deleted)
    }

    #[must_use]
    pub const fn is_hunk(&self) -> bool {
        matches!(self.line.kind, DiffLineKind::Hunk)
    }

    #[must_use]
    pub const fn is_context(&self) -> bool {
        matches!(self.line.kind, DiffLineKind::Context)
    }

    #[must_use]
    pub const fn is_change(&self) -> bool {
        self.is_added() || self.is_deleted()
    }

    #[must_use]
    pub fn is_commentable(&self) -> bool {
        !self.is_hunk()
            && (self.line.old_line_number.is_some() || self.line.new_line_number.is_some())
    }

    #[must_use]
    pub fn can_open_focused_comment(&self) -> bool {
        self.is_focused_comment_mode && self.is_commentable()
    }

    #[must_use]
    pub fn show_normal_comment_gutter(&self) -> bool {
        self.is_commentable()
            && !self.is_focused_comment_mode
            && self.is_selected
            && !self.has_fold()
    }

    #[must_use]
    pub fn comment_side(&self) -> ReviewCommentSide {
        self.display_side
            .unwrap_or(if self.line.kind == DiffLineKind::Deleted {
                ReviewCommentSide::Left
            } else {
                ReviewCommentSide::Right
            })
    }

    #[must_use]
    pub fn line_number_for_side(&self, side: ReviewCommentSide) -> Option<usize> {
        match side {
            ReviewCommentSide::Left => self.line.old_line_number,
            ReviewCommentSide::Right => self.line.new_line_number.or(self.line.old_line_number),
        }
    }

    #[must_use]
    pub fn display_line_number(&self) -> String {
        match self.display_side {
            Some(ReviewCommentSide::Left) => self.old_line_number(),
            Some(ReviewCommentSide::Right) => self.new_line_number(),
            None => self
                .line
                .new_line_number
                .or(self.line.old_line_number)
                .map_or_else(String::new, |line| line.to_string()),
        }
    }

    #[must_use]
    pub fn show_old_line_number(&self) -> bool {
        self.is_commentable() && self.display_side != Some(ReviewCommentSide::Right)
    }

    #[must_use]
    pub fn show_new_line_number(&self) -> bool {
        self.is_commentable() && self.display_side != Some(ReviewCommentSide::Left)
    }

    #[must_use]
    pub fn can_navigate(&self) -> bool {
        !self.is_hunk() && !self.navigation_symbols.is_empty()
    }

    #[must_use]
    pub fn primary_navigation_symbol(&self) -> Option<&SymbolLinkState> {
        self.navigation_symbols.first()
    }

    #[must_use]
    pub fn has_fold(&self) -> bool {
        self.fold_region.is_some()
    }

    #[must_use]
    pub fn fold_line_count(&self) -> usize {
        self.fold_region.as_ref().map_or(0, |region| {
            region.end_index.saturating_sub(region.start_index)
        })
    }

    #[must_use]
    pub fn comment_count(&self) -> usize {
        self.inline_comment_ids.len()
    }

    #[must_use]
    pub fn has_comments(&self) -> bool {
        !self.inline_comment_ids.is_empty()
    }

    #[must_use]
    pub fn selected_fragment(&self) -> String {
        slice_utf16(&self.line.text, self.selection_start, self.selection_length)
    }

    pub fn select_code_range(&mut self, start: usize, length: usize) -> String {
        let units = utf16_length(&self.line.text);
        self.selection_start = start.min(units);
        self.selection_length = length.min(units.saturating_sub(self.selection_start));
        self.selected_fragment()
    }

    pub fn clear_code_selection(&mut self) {
        self.selection_start = 0;
        self.selection_length = 0;
    }

    pub fn configure_fold(&mut self, fold: Option<CodeFoldRegion>, collapsed: bool) {
        self.fold_region = fold;
        self.is_fold_collapsed = collapsed && self.fold_region.is_some();
    }

    pub fn set_search_state(&mut self, is_match: bool, is_current: bool, query: &str) {
        self.is_search_match = is_match;
        self.is_current_search_match = is_match && is_current;
        self.search_query = if is_match {
            query.trim().to_string()
        } else {
            String::new()
        };
    }

    pub fn set_difference(&mut self, counterpart: &str) {
        let ours = self.line.text.chars().collect::<Vec<_>>();
        let theirs = counterpart.chars().collect::<Vec<_>>();
        let mut prefix = 0;
        let maximum = ours.len().min(theirs.len());
        while prefix < maximum && ours[prefix] == theirs[prefix] {
            prefix += 1;
        }
        let mut suffix = 0;
        while suffix < maximum.saturating_sub(prefix)
            && ours[ours.len() - 1 - suffix] == theirs[theirs.len() - 1 - suffix]
        {
            suffix += 1;
        }
        let difference_end = ours.len().saturating_sub(suffix);
        self.difference_prefix = ours[..prefix].iter().collect();
        self.difference_text = ours[prefix..difference_end].iter().collect();
        self.difference_suffix = ours[difference_end..].iter().collect();
    }

    pub fn clear_difference(&mut self) {
        self.difference_prefix.clear();
        self.difference_text.clear();
        self.difference_suffix.clear();
    }

    #[must_use]
    pub fn display_runs(&self) -> Vec<DiffCodeTextRun> {
        if self.is_search_match && !self.search_query.is_empty() {
            let ranges = find_case_insensitive_ranges(&self.line.text, &self.search_query);
            if !ranges.is_empty() {
                return text_runs_for_ranges(
                    &self.line.text,
                    ranges
                        .into_iter()
                        .map(|(start, end)| (start, end, DiffHighlightKind::Search)),
                );
            }
        }

        if !self.difference_text.is_empty() {
            let start = self.difference_prefix.len();
            return text_runs_for_ranges(
                &self.line.text,
                std::iter::once((
                    start,
                    start + self.difference_text.len(),
                    if self.is_added() {
                        DiffHighlightKind::Added
                    } else {
                        DiffHighlightKind::Deleted
                    },
                )),
            );
        }

        vec![DiffCodeTextRun {
            text: self.line.text.clone(),
            kind: DiffHighlightKind::None,
        }]
    }

    #[must_use]
    pub fn with_display_side(&self, side: ReviewCommentSide) -> Self {
        let mut copy = self.clone();
        copy.display_side = Some(side);
        copy.inline_comment_ids = match side {
            ReviewCommentSide::Left => copy.left_comment_ids.clone(),
            ReviewCommentSide::Right => copy.right_comment_ids.clone(),
        };
        copy.navigation_symbols = extract_symbol_candidates(&copy.line.text)
            .iter()
            .filter(|candidate| !candidate.is_likely_external)
            .map(|candidate| {
                let mut symbol = SymbolLinkState::from(candidate);
                symbol.leading_offset =
                    8.0_f64.max((candidate.column_index as f64 * 7.8).min(320.0));
                symbol
            })
            .collect();
        copy
    }
}

impl From<&DiffLine> for DiffLineState {
    fn from(line: &DiffLine) -> Self {
        Self::from_ref(line, 0)
    }
}

impl From<DiffLine> for DiffLineState {
    fn from(line: DiffLine) -> Self {
        Self::new(line)
    }
}

fn utf16_length(value: &str) -> usize {
    value.encode_utf16().count()
}

fn slice_utf16(value: &str, start: usize, length: usize) -> String {
    if length == 0 {
        return String::new();
    }
    let end = start.saturating_add(length);
    let mut result = String::new();
    let mut position = 0;
    for character in value.chars() {
        let character_end = position + character.len_utf16();
        if character_end > start && position < end {
            result.push(character);
        }
        position = character_end;
        if position >= end {
            break;
        }
    }
    result
}

fn find_case_insensitive_ranges(value: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    let query_characters = query.chars().count();
    let boundaries = value
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(value.len()))
        .collect::<Vec<_>>();
    let mut ranges = Vec::new();
    let mut start_character = 0;
    while start_character + query_characters <= boundaries.len().saturating_sub(1) {
        let start = boundaries[start_character];
        let end = boundaries[start_character + query_characters];
        if value[start..end].eq_ignore_ascii_case(query) {
            ranges.push((start, end));
            start_character += query_characters;
        } else {
            start_character += 1;
        }
    }
    ranges
}

fn text_runs_for_ranges(
    text: &str,
    ranges: impl IntoIterator<Item = (usize, usize, DiffHighlightKind)>,
) -> Vec<DiffCodeTextRun> {
    let mut ranges = ranges.into_iter().collect::<Vec<_>>();
    ranges.sort_by_key(|(start, end, _)| (*start, *end));
    let mut runs = Vec::new();
    let mut position = 0;
    for (start, end, kind) in ranges {
        let start = start.min(text.len());
        let end = end.min(text.len()).max(start);
        if start > position {
            runs.push(DiffCodeTextRun {
                text: text[position..start].to_string(),
                kind: DiffHighlightKind::None,
            });
        }
        if end > start {
            runs.push(DiffCodeTextRun {
                text: text[start..end].to_string(),
                kind,
            });
        }
        position = position.max(end);
    }
    if position < text.len() {
        runs.push(DiffCodeTextRun {
            text: text[position..].to_string(),
            kind: DiffHighlightKind::None,
        });
    }
    if runs.is_empty() {
        runs.push(DiffCodeTextRun {
            text: text.to_string(),
            kind: DiffHighlightKind::None,
        });
    }
    runs
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffRowState {
    pub inline: Option<DiffLineState>,
    pub hunk: Option<DiffLineState>,
    pub left: Option<DiffLineState>,
    pub right: Option<DiffLineState>,
}

impl DiffRowState {
    #[must_use]
    pub fn inline(line: DiffLineState) -> Self {
        Self {
            hunk: line.is_hunk().then(|| line.clone()),
            inline: (!line.is_hunk()).then_some(line),
            left: None,
            right: None,
        }
    }

    #[must_use]
    pub fn side_by_side(left: Option<DiffLineState>, right: Option<DiffLineState>) -> Self {
        Self {
            inline: None,
            hunk: None,
            left,
            right,
        }
    }

    #[must_use]
    pub fn hunk(line: DiffLineState) -> Self {
        Self {
            inline: None,
            hunk: Some(line),
            left: None,
            right: None,
        }
    }

    #[must_use]
    pub fn is_inline(&self) -> bool {
        self.inline.is_some()
    }

    #[must_use]
    pub fn is_hunk(&self) -> bool {
        self.hunk.is_some()
    }

    #[must_use]
    pub fn is_side_by_side(&self) -> bool {
        !self.is_inline() && !self.is_hunk()
    }

    #[must_use]
    pub fn has_left(&self) -> bool {
        self.left.is_some()
    }

    #[must_use]
    pub fn has_right(&self) -> bool {
        self.right.is_some()
    }

    #[must_use]
    pub fn contains_source_index(&self, index: usize) -> bool {
        self.inline
            .as_ref()
            .into_iter()
            .chain(self.hunk.as_ref())
            .chain(self.left.as_ref())
            .chain(self.right.as_ref())
            .any(|line| line.source_index == index)
    }

    #[must_use]
    pub fn contains(&self, line: &DiffLineState) -> bool {
        self.contains_source_index(line.source_index)
            && [
                self.inline.as_ref(),
                self.hunk.as_ref(),
                self.left.as_ref(),
                self.right.as_ref(),
            ]
            .into_iter()
            .flatten()
            .any(|candidate| candidate.line == line.line)
    }

    #[must_use]
    pub fn hunk_text(&self) -> &str {
        self.hunk.as_ref().map_or("", DiffLineState::code_text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffLayout {
    #[default]
    Inline,
    SideBySide,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffChangeMarkerState {
    pub line_index: usize,
    pub relative_position: f64,
    pub is_addition: bool,
    pub is_comment: bool,
}

impl Default for DiffChangeMarkerState {
    fn default() -> Self {
        Self {
            line_index: 0,
            relative_position: 0.0,
            is_addition: false,
            is_comment: false,
        }
    }
}

impl DiffChangeMarkerState {
    #[must_use]
    pub fn label(&self, line: &DiffLineState) -> String {
        let number = line
            .line
            .new_line_number
            .or(line.line.old_line_number)
            .unwrap_or(1);
        if self.is_comment {
            format!("Comment at {}:{number}", line.line.file_path)
        } else {
            format!("{}:{number}", line.line.file_path)
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct DiffState {
    pub lines: Vec<DiffLineState>,
    pub visible_line_indices: Vec<usize>,
    pub rows: Vec<DiffRowState>,
    pub layout: DiffLayout,
    pub is_focused_comment_mode: bool,
    pub fold_regions: Vec<CodeFoldRegion>,
    pub collapsed_fold_starts: BTreeSet<usize>,
    pub search_query: String,
    pub search_match_indices: Vec<usize>,
    pub current_search_match: Option<usize>,
    pub selected_line_index: Option<usize>,
    pub change_indices: Vec<usize>,
    pub change_markers: Vec<DiffChangeMarkerState>,
    pub is_folder_selection: bool,
    pub show_change_markers: bool,
    pub allow_folds: bool,
}

impl Default for DiffState {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            visible_line_indices: Vec::new(),
            rows: Vec::new(),
            layout: DiffLayout::Inline,
            is_focused_comment_mode: false,
            fold_regions: Vec::new(),
            collapsed_fold_starts: BTreeSet::new(),
            search_query: String::new(),
            search_match_indices: Vec::new(),
            current_search_match: None,
            selected_line_index: None,
            change_indices: Vec::new(),
            change_markers: Vec::new(),
            is_folder_selection: false,
            show_change_markers: true,
            allow_folds: true,
        }
    }
}

impl DiffState {
    #[must_use]
    pub fn from_lines(lines: impl IntoIterator<Item = DiffLine>) -> Self {
        let mut state = Self::default();
        state.replace_lines(lines, false);
        state
    }

    #[must_use]
    pub fn from_blocks(blocks: &[DiffBlock], is_folder_selection: bool) -> Self {
        let mut state = Self::default();
        state.replace_blocks(blocks, is_folder_selection);
        state
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.visible_line_indices.clear();
        self.rows.clear();
        self.fold_regions.clear();
        self.collapsed_fold_starts.clear();
        self.search_match_indices.clear();
        self.current_search_match = None;
        self.selected_line_index = None;
        self.change_indices.clear();
        self.change_markers.clear();
        self.search_query.clear();
        self.is_folder_selection = false;
    }

    pub fn replace_lines(
        &mut self,
        lines: impl IntoIterator<Item = DiffLine>,
        is_folder_selection: bool,
    ) {
        let focused_comment_mode = self.is_focused_comment_mode;
        self.lines = lines
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                let mut line = DiffLineState::from_line(line, index);
                line.is_focused_comment_mode = focused_comment_mode;
                line
            })
            .collect();
        self.is_folder_selection = is_folder_selection;
        self.collapsed_fold_starts.clear();
        self.search_query.clear();
        self.search_match_indices.clear();
        self.current_search_match = None;
        self.change_indices = self
            .lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| line.is_change().then_some(index))
            .collect();
        self.apply_inline_differences();
        self.rebuild_folds();
        self.selected_line_index = self.change_indices.first().copied();
        for line in &mut self.lines {
            line.is_selected = self.selected_line_index == Some(line.source_index);
        }
        self.rebuild_projection();
    }

    pub fn replace_blocks(&mut self, blocks: &[DiffBlock], is_folder_selection: bool) {
        let mut lines = Vec::new();
        for block in blocks {
            lines.push(DiffLine {
                kind: DiffLineKind::Hunk,
                file_path: block.file_path.clone(),
                text: block.previous_path.as_ref().map_or_else(
                    || block.file_path.clone(),
                    |previous| format!("{previous} -> {}", block.file_path),
                ),
                ..Default::default()
            });
            lines.extend(block.lines.iter().cloned());
        }
        self.replace_lines(lines, is_folder_selection);
    }

    pub fn set_layout(&mut self, layout: DiffLayout) {
        self.layout = layout;
        self.rebuild_rows();
    }

    pub fn set_focused_comment_mode(&mut self, enabled: bool) {
        self.is_focused_comment_mode = enabled;
        for line in &mut self.lines {
            line.is_focused_comment_mode = enabled;
        }
        self.rebuild_rows();
    }

    pub fn set_marker_eligibility(&mut self, eligible: bool) {
        self.show_change_markers = eligible;
        self.rebuild_markers(eligible);
    }

    #[must_use]
    pub fn is_inline(&self) -> bool {
        self.layout == DiffLayout::Inline
    }

    #[must_use]
    pub fn is_side_by_side(&self) -> bool {
        self.layout == DiffLayout::SideBySide
    }

    #[must_use]
    pub fn has_function_folds(&self) -> bool {
        !self.fold_regions.is_empty() && !self.is_folder_selection
    }

    #[must_use]
    pub fn can_search(&self) -> bool {
        !self.is_folder_selection && !self.lines.is_empty()
    }

    #[must_use]
    pub fn selected_line(&self) -> Option<&DiffLineState> {
        self.selected_line_index
            .and_then(|index| self.lines.get(index))
    }

    #[must_use]
    pub fn selected_line_mut(&mut self) -> Option<&mut DiffLineState> {
        self.selected_line_index
            .and_then(|index| self.lines.get_mut(index))
    }

    pub fn select_line(&mut self, line_index: usize) -> bool {
        if line_index >= self.lines.len() {
            return false;
        }
        for line in &mut self.lines {
            line.is_selected = false;
        }
        self.lines[line_index].is_selected = true;
        self.selected_line_index = Some(line_index);
        self.ensure_line_visible(line_index);
        self.rebuild_rows();
        true
    }

    pub fn clear_selected_line(&mut self) {
        for line in &mut self.lines {
            line.is_selected = false;
        }
        self.selected_line_index = None;
        self.rebuild_rows();
    }

    #[must_use]
    pub fn change_count(&self) -> usize {
        self.change_indices.len()
    }

    #[must_use]
    pub fn change_position(&self) -> Option<usize> {
        let selected = self.selected_line_index?;
        self.change_indices
            .iter()
            .position(|index| *index == selected)
            .map(|index| index + 1)
    }

    #[must_use]
    pub fn next_change_index(&self, direction: isize) -> Option<usize> {
        if self.change_indices.is_empty() {
            return None;
        }
        let current = self.selected_line_index.and_then(|selected| {
            self.change_indices
                .iter()
                .position(|index| *index == selected)
        });
        let target = if direction >= 0 {
            current.map_or(0, |index| (index + 1) % self.change_indices.len())
        } else {
            current.map_or(self.change_indices.len() - 1, |index| {
                (index + self.change_indices.len() - 1) % self.change_indices.len()
            })
        };
        self.change_indices.get(target).copied()
    }

    pub fn navigate_change(&mut self, direction: isize) -> Option<usize> {
        let target = self.next_change_index(direction)?;
        self.select_line(target);
        Some(target)
    }

    pub fn set_search_query(&mut self, query: impl Into<String>) {
        self.search_query = query.into().trim().to_string();
        self.recompute_search();
    }

    #[must_use]
    pub fn has_search_query(&self) -> bool {
        !self.search_query.trim().is_empty()
    }

    #[must_use]
    pub fn has_search_matches(&self) -> bool {
        !self.search_match_indices.is_empty()
    }

    #[must_use]
    pub fn search_position(&self) -> Option<usize> {
        self.current_search_match.map(|index| index + 1)
    }

    pub fn navigate_search(&mut self, direction: isize) -> Option<usize> {
        if self.search_match_indices.is_empty() {
            return None;
        }
        for line in &mut self.lines {
            line.is_current_search_match = false;
        }
        let length = self.search_match_indices.len();
        let target = match self.current_search_match {
            Some(current) if direction >= 0 => (current + 1) % length,
            Some(current) => (current + length - 1) % length,
            None if direction >= 0 => 0,
            None => length - 1,
        };
        self.current_search_match = Some(target);
        let line_index = self.search_match_indices[target];
        self.lines[line_index].is_current_search_match = true;
        self.select_line(line_index);
        self.lines[line_index].is_current_search_match = true;
        Some(line_index)
    }

    pub fn toggle_fold(&mut self, start_index: usize) -> bool {
        if !self
            .fold_regions
            .iter()
            .any(|region| region.start_index == start_index)
        {
            return false;
        }
        if !self.collapsed_fold_starts.insert(start_index) {
            self.collapsed_fold_starts.remove(&start_index);
        }
        self.rebuild_projection();
        true
    }

    pub fn collapse_all_folds(&mut self) -> usize {
        self.collapsed_fold_starts = self
            .fold_regions
            .iter()
            .map(|region| region.start_index)
            .collect();
        self.rebuild_projection();
        self.collapsed_fold_starts.len()
    }

    pub fn expand_all_folds(&mut self) {
        self.collapsed_fold_starts.clear();
        self.rebuild_projection();
    }

    pub fn ensure_line_visible(&mut self, line_index: usize) -> bool {
        if line_index >= self.lines.len() {
            return false;
        }
        let mut changed = false;
        let containing = self
            .fold_regions
            .iter()
            .filter(|region| line_index > region.start_index && line_index <= region.end_index)
            .map(|region| region.start_index)
            .collect::<Vec<_>>();
        for start in containing {
            changed |= self.collapsed_fold_starts.remove(&start);
        }
        if changed {
            self.rebuild_projection();
        }
        true
    }

    #[must_use]
    pub fn visible_lines(&self) -> Vec<&DiffLineState> {
        self.visible_line_indices
            .iter()
            .filter_map(|index| self.lines.get(*index))
            .collect()
    }

    #[must_use]
    pub fn find_row(&self, source_index: usize) -> Option<&DiffRowState> {
        self.rows
            .iter()
            .find(|row| row.contains_source_index(source_index))
    }

    pub fn set_line_comment_ids(&mut self, line_index: usize, ids: Vec<String>) {
        if let Some(line) = self.lines.get_mut(line_index) {
            line.inline_comment_ids = ids;
            self.rebuild_markers(self.show_change_markers);
            self.rebuild_rows();
        }
    }

    pub fn set_line_comment_ids_by_side(
        &mut self,
        line_index: usize,
        left_ids: Vec<String>,
        right_ids: Vec<String>,
    ) {
        if let Some(line) = self.lines.get_mut(line_index) {
            line.left_comment_ids = left_ids;
            line.right_comment_ids = right_ids;
            let mut all = line.left_comment_ids.clone();
            all.extend(line.right_comment_ids.iter().cloned());
            all.sort();
            all.dedup();
            line.inline_comment_ids = all;
        }
    }

    pub fn rebuild_markers(&mut self, include_changes: bool) {
        self.change_markers.clear();
        if !include_changes {
            return;
        }
        let visible_count = self.visible_line_indices.len().max(1);
        for &line_index in &self.change_indices {
            if line_index > 0
                && self
                    .lines
                    .get(line_index - 1)
                    .is_some_and(DiffLineState::is_change)
            {
                continue;
            }
            let visible_index = self
                .visible_line_indices
                .iter()
                .position(|index| *index == line_index);
            self.change_markers.push(DiffChangeMarkerState {
                line_index,
                relative_position: visible_index
                    .map_or(0.0, |index| index as f64 / visible_count as f64),
                is_addition: self.lines[line_index].is_added(),
                is_comment: false,
            });
        }
        for &line_index in &self.visible_line_indices {
            let line = &self.lines[line_index];
            if !line.has_comments() {
                continue;
            }
            let visible_index = self
                .visible_line_indices
                .iter()
                .position(|index| *index == line_index)
                .expect("the visible index came from visible_line_indices");
            self.change_markers.push(DiffChangeMarkerState {
                line_index,
                relative_position: visible_index as f64 / visible_count as f64,
                is_addition: false,
                is_comment: true,
            });
        }
    }

    fn apply_inline_differences(&mut self) {
        for line in &mut self.lines {
            line.clear_difference();
        }
        let models = self
            .lines
            .iter()
            .map(|line| line.line.clone())
            .collect::<Vec<_>>();
        let pairs = build_side_by_side(&models);
        let mut used = BTreeSet::new();
        for pair in pairs {
            if !matches!(
                (
                    pair.left_line.as_ref().map(|line| line.kind),
                    pair.right_line.as_ref().map(|line| line.kind),
                ),
                (Some(DiffLineKind::Deleted), Some(DiffLineKind::Added))
            ) {
                continue;
            }
            let left_index = claim_line_index(&self.lines, &mut used, pair.left_line.as_ref());
            let right_index = claim_line_index(&self.lines, &mut used, pair.right_line.as_ref());
            if let (Some(left_index), Some(right_index)) = (left_index, right_index) {
                let left_text = self.lines[left_index].line.text.clone();
                let right_text = self.lines[right_index].line.text.clone();
                self.lines[left_index].set_difference(&right_text);
                self.lines[right_index].set_difference(&left_text);
            }
        }
    }

    fn rebuild_folds(&mut self) {
        let models = self
            .lines
            .iter()
            .map(|line| line.line.clone())
            .collect::<Vec<_>>();
        self.fold_regions = if self.is_folder_selection || !self.allow_folds {
            Vec::new()
        } else {
            build_folds(&models).into_iter().map(Into::into).collect()
        };
        for line in &mut self.lines {
            line.configure_fold(None, false);
        }
        for region in &self.fold_regions {
            if let Some(line) = self.lines.get_mut(region.start_index) {
                line.configure_fold(
                    Some(region.clone()),
                    self.collapsed_fold_starts.contains(&region.start_index),
                );
            }
        }
    }

    fn recompute_search(&mut self) {
        for line in &mut self.lines {
            line.set_search_state(false, false, "");
        }
        self.search_match_indices.clear();
        self.current_search_match = None;
        if self.can_search() && !self.search_query.is_empty() {
            for (index, line) in self.lines.iter_mut().enumerate() {
                let is_match = line.line.kind != DiffLineKind::Hunk
                    && !find_case_insensitive_ranges(&line.line.text, &self.search_query)
                        .is_empty();
                if is_match {
                    line.set_search_state(true, false, &self.search_query);
                    self.search_match_indices.push(index);
                }
            }
        }
        if !self.search_match_indices.is_empty() {
            self.navigate_search(1);
        }
        self.rebuild_rows();
    }

    fn rebuild_projection(&mut self) {
        let mut visible = Vec::new();
        let mut index = 0;
        while index < self.lines.len() {
            visible.push(index);
            if self.collapsed_fold_starts.contains(&index)
                && let Some(region) = self
                    .fold_regions
                    .iter()
                    .find(|region| region.start_index == index)
            {
                index = region.end_index.min(self.lines.len().saturating_sub(1));
            }
            index += 1;
        }
        self.visible_line_indices = visible;
        self.rebuild_folds();
        self.rebuild_rows();
        self.rebuild_markers(self.show_change_markers);
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        if self.is_inline() {
            self.rows = self
                .visible_line_indices
                .iter()
                .filter_map(|index| self.lines.get(*index))
                .cloned()
                .map(DiffRowState::inline)
                .collect();
            return;
        }

        let visible_models = self
            .visible_line_indices
            .iter()
            .filter_map(|index| self.lines.get(*index))
            .map(|line| line.line.clone())
            .collect::<Vec<_>>();
        let pairs = build_side_by_side(&visible_models);
        let mut claimed = BTreeSet::new();
        for pair in pairs {
            if pair.hunk_line.is_some() {
                let index = claim_visible_line_index(
                    &self.lines,
                    &self.visible_line_indices,
                    pair.hunk_line.as_ref(),
                    &mut claimed,
                );
                if let Some(index) = index {
                    self.rows
                        .push(DiffRowState::hunk(self.lines[index].clone()));
                }
                continue;
            }

            let left_index = pair.left_line.as_ref().and_then(|left| {
                claim_visible_line_index(
                    &self.lines,
                    &self.visible_line_indices,
                    Some(left),
                    &mut claimed,
                )
            });
            let right_index = if pair.left_line == pair.right_line {
                left_index
            } else {
                pair.right_line.as_ref().and_then(|right| {
                    claim_visible_line_index(
                        &self.lines,
                        &self.visible_line_indices,
                        Some(right),
                        &mut claimed,
                    )
                })
            };
            self.rows.push(DiffRowState::side_by_side(
                left_index
                    .map(|index| self.lines[index].with_display_side(ReviewCommentSide::Left)),
                right_index
                    .map(|index| self.lines[index].with_display_side(ReviewCommentSide::Right)),
            ));
        }
    }
}

fn claim_line_index(
    lines: &[DiffLineState],
    used: &mut BTreeSet<usize>,
    target: Option<&DiffLine>,
) -> Option<usize> {
    let target = target?;
    let index = lines
        .iter()
        .position(|line| line.line == *target && !used.contains(&line.source_index))?;
    used.insert(lines[index].source_index);
    Some(index)
}

fn claim_visible_line_index(
    lines: &[DiffLineState],
    visible: &[usize],
    target: Option<&DiffLine>,
    used: &mut BTreeSet<usize>,
) -> Option<usize> {
    let target = target?;
    let index = visible.iter().copied().find(|index| {
        !used.contains(index) && lines.get(*index).is_some_and(|line| line.line == *target)
    })?;
    used.insert(index);
    Some(index)
}

#[derive(Clone, PartialEq, Eq)]
pub struct LocalReviewCommentState {
    pub comment: ReviewComment,
    pub edit_body: String,
    pub edit_suggestion: String,
    pub is_editing: bool,
    pub is_expanded: bool,
}

impl Default for LocalReviewCommentState {
    fn default() -> Self {
        Self::new(ReviewComment::default())
    }
}

impl fmt::Debug for LocalReviewCommentState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalReviewCommentState")
            .field("id", &self.comment.id)
            .field("file_path", &self.comment.file_path)
            .field("line_number", &self.comment.line_number)
            .field("status", &self.comment.status)
            .field("is_editing", &self.is_editing)
            .finish_non_exhaustive()
    }
}

impl LocalReviewCommentState {
    #[must_use]
    pub fn new(comment: ReviewComment) -> Self {
        Self {
            comment,
            edit_body: String::new(),
            edit_suggestion: String::new(),
            is_editing: false,
            is_expanded: true,
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.comment.id
    }

    #[must_use]
    pub fn author(&self) -> &'static str {
        "You"
    }

    #[must_use]
    pub fn location(&self) -> String {
        if self.comment.file_path.is_empty() {
            String::new()
        } else {
            format!("{}:{}", self.comment.file_path, self.comment.line_number)
        }
    }

    #[must_use]
    pub fn display_body(&self) -> String {
        markdown::to_plain_text(&self.comment.body)
    }

    #[must_use]
    pub fn is_pending(&self) -> bool {
        matches!(
            self.comment.status,
            ReviewCommentStatus::Pending | ReviewCommentStatus::Failed
        )
    }

    #[must_use]
    pub fn is_draft(&self) -> bool {
        self.is_pending()
    }

    #[must_use]
    pub fn is_published(&self) -> bool {
        self.comment.status == ReviewCommentStatus::Published
    }

    #[must_use]
    pub fn can_edit(&self) -> bool {
        self.is_draft()
    }

    #[must_use]
    pub fn can_save_edit(&self) -> bool {
        self.is_editing
            && (!self.edit_body.trim().is_empty() || !self.edit_suggestion.trim().is_empty())
    }

    #[must_use]
    pub fn has_error(&self) -> bool {
        self.comment
            .error
            .as_deref()
            .is_some_and(|error| !error.trim().is_empty())
    }

    #[must_use]
    pub fn matches_file(&self, path: &str) -> bool {
        path.trim().is_empty() || paths_equal(&self.comment.file_path, path)
    }

    pub fn begin_edit(&mut self) -> bool {
        if !self.can_edit() {
            return false;
        }
        self.edit_body.clone_from(&self.comment.body);
        self.edit_suggestion
            .clone_from(&self.comment.suggested_replacement);
        self.is_editing = true;
        true
    }

    pub fn cancel_edit(&mut self) {
        self.is_editing = false;
        self.edit_body.clear();
        self.edit_suggestion.clear();
    }

    pub fn commit_edit(&mut self) -> bool {
        if !self.can_save_edit() {
            return false;
        }
        let body = self.edit_body.trim().to_string();
        self.comment.body = if body.is_empty() {
            "Suggested change.".to_string()
        } else {
            body
        };
        self.comment.suggested_replacement = self.edit_suggestion.trim_end().to_string();
        self.comment.error = None;
        self.is_editing = false;
        self.edit_body.clear();
        self.edit_suggestion.clear();
        true
    }

    pub fn toggle_expanded(&mut self) {
        self.is_expanded = !self.is_expanded;
    }
}

impl From<&ReviewComment> for LocalReviewCommentState {
    fn from(comment: &ReviewComment) -> Self {
        Self::new(comment.clone())
    }
}

impl From<ReviewComment> for LocalReviewCommentState {
    fn from(comment: ReviewComment) -> Self {
        Self::new(comment)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RemoteReviewCommentState {
    pub comment: RemoteReviewComment,
    pub is_expanded: bool,
    pub show_jump_to_line: bool,
}

impl fmt::Debug for RemoteReviewCommentState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteReviewCommentState")
            .field("id", &self.comment.id)
            .field("remote_id", &self.comment.remote_id)
            .field("author", &self.comment.author)
            .field("file_path", &self.comment.file_path)
            .field("line_number", &self.comment.line_number)
            .field("is_resolved", &self.comment.is_resolved)
            .finish_non_exhaustive()
    }
}

impl RemoteReviewCommentState {
    #[must_use]
    pub fn new(comment: RemoteReviewComment, show_jump_to_line: bool) -> Self {
        Self {
            comment,
            is_expanded: true,
            show_jump_to_line,
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        if self.comment.id.is_empty() {
            &self.comment.remote_id
        } else {
            &self.comment.id
        }
    }

    #[must_use]
    pub fn display_body(&self) -> String {
        markdown::to_plain_text(&self.comment.body)
    }

    #[must_use]
    pub fn location(&self) -> String {
        match (self.comment.file_path.trim(), self.comment.line_number) {
            ("", _) => String::new(),
            (path, Some(line)) => format!("{path}:{line}"),
            (path, None) => path.to_string(),
        }
    }

    #[must_use]
    pub fn is_bot(&self) -> bool {
        self.comment.author.to_ascii_lowercase().contains("[bot]")
            || self
                .comment
                .author
                .to_ascii_lowercase()
                .contains("azure-pipelines")
    }

    #[must_use]
    pub fn can_navigate_in_app(&self) -> bool {
        !self.comment.file_path.trim().is_empty()
            && self.comment.line_number.is_some()
            && self.comment.side.is_some()
    }

    #[must_use]
    pub fn can_open_in_provider(&self) -> bool {
        !self.comment.web_url.trim().is_empty() && !self.can_navigate_in_app()
    }

    #[must_use]
    pub fn show_in_app_jump(&self) -> bool {
        self.show_jump_to_line && self.can_navigate_in_app()
    }

    #[must_use]
    pub fn matches_file(&self, path: &str) -> bool {
        path.trim().is_empty() || paths_equal(&self.comment.file_path, path)
    }
}

impl From<&RemoteReviewComment> for RemoteReviewCommentState {
    fn from(comment: &RemoteReviewComment) -> Self {
        Self::new(comment.clone(), true)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RemoteReviewThreadState {
    pub id: String,
    pub root: RemoteReviewCommentState,
    pub replies: Vec<RemoteReviewCommentState>,
    pub reply_text: String,
    pub is_reply_composer_visible: bool,
    pub is_expanded: bool,
}

impl fmt::Debug for RemoteReviewThreadState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteReviewThreadState")
            .field("id", &self.id)
            .field("root_id", &self.root.id())
            .field("reply_count", &self.replies.len())
            .field("is_reply_composer_visible", &self.is_reply_composer_visible)
            .finish_non_exhaustive()
    }
}

impl RemoteReviewThreadState {
    #[must_use]
    pub fn has_replies(&self) -> bool {
        !self.replies.is_empty()
    }

    #[must_use]
    pub fn thread_comment_count(&self) -> usize {
        1 + self.replies.len()
    }

    #[must_use]
    pub fn is_bot(&self) -> bool {
        self.root.is_bot()
    }

    #[must_use]
    pub fn matches_file(&self, path: &str) -> bool {
        self.root.matches_file(path) || self.replies.iter().any(|reply| reply.matches_file(path))
    }

    #[must_use]
    pub fn can_reply(&self) -> bool {
        true
    }

    #[must_use]
    pub fn can_submit_reply(&self) -> bool {
        self.can_reply() && !self.reply_text.trim().is_empty()
    }

    #[must_use]
    pub fn reply_draft(&self) -> Option<&str> {
        self.can_submit_reply().then_some(self.reply_text.as_str())
    }

    pub fn begin_reply(&mut self) {
        self.is_reply_composer_visible = true;
    }

    pub fn cancel_reply(&mut self) {
        self.reply_text.clear();
        self.is_reply_composer_visible = false;
    }

    pub fn complete_reply(&mut self) {
        self.cancel_reply();
    }

    pub fn toggle_expanded(&mut self) {
        self.is_expanded = !self.is_expanded;
    }
}

pub fn build_remote_comment_threads(
    comments: &[RemoteReviewComment],
    show_jump_to_line: bool,
) -> Vec<RemoteReviewThreadState> {
    let mut comments = comments.to_vec();
    comments.sort_by_key(|comment| std::cmp::Reverse(comment.created_at_utc));
    let mut grouped = Vec::<(String, Vec<RemoteReviewComment>)>::new();
    for comment in comments {
        let key = if comment.thread_id.trim().is_empty() {
            if comment.remote_id.trim().is_empty() {
                comment.id.clone()
            } else {
                comment.remote_id.clone()
            }
        } else {
            comment.thread_id.clone()
        };
        if let Some((_, group)) = grouped.iter_mut().find(|(candidate, _)| candidate == &key) {
            group.push(comment);
        } else {
            grouped.push((key, vec![comment]));
        }
    }

    grouped
        .into_iter()
        .filter_map(|(id, mut group)| {
            group.sort_by_key(|comment| comment.created_at_utc);
            let root_index = group
                .iter()
                .position(|comment| {
                    comment
                        .parent_remote_id
                        .as_deref()
                        .is_none_or(|parent| parent.trim().is_empty())
                })
                .unwrap_or(0);
            let root = group.remove(root_index);
            if root.remote_id.trim().is_empty() && id.trim().is_empty() {
                return None;
            }
            Some(RemoteReviewThreadState {
                id,
                root: RemoteReviewCommentState::new(root, show_jump_to_line),
                replies: group
                    .into_iter()
                    .map(|comment| RemoteReviewCommentState::new(comment, show_jump_to_line))
                    .collect(),
                reply_text: String::new(),
                is_reply_composer_visible: false,
                is_expanded: true,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InspectorTab {
    #[default]
    Comments,
    Overview,
    Notes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NotesSaveState {
    #[default]
    Saved,
    Saving,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRangeState {
    pub iteration_id: Option<String>,
    pub base_commit_sha: String,
    pub head_commit_sha: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ReviewActionReadiness {
    pub can_refresh: bool,
    pub can_mark_reviewed: bool,
    pub can_save_notes: bool,
    pub can_save_comment: bool,
    pub can_publish_comments: bool,
    pub can_search_file: bool,
    pub can_move_search_previous: bool,
    pub can_move_search_next: bool,
    pub can_move_change_previous: bool,
    pub can_move_change_next: bool,
    pub can_open_comment: bool,
}

impl Default for ReviewActionReadiness {
    fn default() -> Self {
        Self::all_false()
    }
}

impl ReviewActionReadiness {
    #[must_use]
    pub const fn all_false() -> Self {
        Self {
            can_refresh: false,
            can_mark_reviewed: false,
            can_save_notes: false,
            can_save_comment: false,
            can_publish_comments: false,
            can_search_file: false,
            can_move_search_previous: false,
            can_move_search_next: false,
            can_move_change_previous: false,
            can_move_change_next: false,
            can_open_comment: false,
        }
    }
}

impl fmt::Debug for ReviewActionReadiness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReviewActionReadiness")
            .field("can_refresh", &self.can_refresh)
            .field("can_mark_reviewed", &self.can_mark_reviewed)
            .field("can_save_notes", &self.can_save_notes)
            .field("can_save_comment", &self.can_save_comment)
            .field("can_publish_comments", &self.can_publish_comments)
            .field("can_open_comment", &self.can_open_comment)
            .finish_non_exhaustive()
    }
}

pub struct ReviewActionState;

impl ReviewActionState {
    #[must_use]
    pub fn can_save_comment(
        has_session: bool,
        has_commentable_line: bool,
        comment: &str,
        suggestion: &str,
        is_busy: bool,
    ) -> bool {
        has_session
            && has_commentable_line
            && !is_busy
            && (!comment.trim().is_empty() || !suggestion.trim().is_empty())
    }

    #[must_use]
    pub fn can_publish_comments(pending_count: usize, is_busy: bool) -> bool {
        pending_count > 0 && !is_busy
    }

    #[must_use]
    pub fn can_reply(body: &str, is_busy: bool) -> bool {
        !is_busy && !body.trim().is_empty()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DefinitionNavigationState {
    pub symbol: String,
    pub location: SymbolLocation,
    pub heading: String,
}

#[derive(Clone, PartialEq)]
pub struct ReviewSessionAppState {
    pub session: Option<ReviewSessionState>,
    pub selected_file_path: String,
    pub selected_path_is_folder: bool,
    pub selected_iteration_id: Option<String>,
    pub iterations: Vec<PullRequestIteration>,
    pub file_roots: Vec<FileTreeNodeState>,
    pub viewed_files: Vec<String>,
    pub file_filter: String,
    pub diff: DiffState,
    pub context_lines: u32,
    pub inspector_tab: InspectorTab,
    pub notes: String,
    pub notes_save_state: NotesSaveState,
    pub draft_comment: String,
    pub draft_suggestion: String,
    pub selected_code_fragment: String,
    pub focused_suggestion_seed: String,
    pub comments: Vec<LocalReviewCommentState>,
    pub remote_threads: Vec<RemoteReviewThreadState>,
    pub show_only_current_file_comments: bool,
    pub hide_bot_comments: bool,
    pub is_markdown_formatted: bool,
    pub markdown_blocks: Vec<MarkdownBlock>,
    pub description_markdown_blocks: Vec<MarkdownBlock>,
    pub is_busy: bool,
    pub error_message: String,
    pub status_message: String,
    pub is_viewing_definition: bool,
    pub definition_navigation: Option<DefinitionNavigationState>,
    saved_diff: Option<DiffState>,
}

impl Default for ReviewSessionAppState {
    fn default() -> Self {
        Self {
            session: None,
            selected_file_path: String::new(),
            selected_path_is_folder: false,
            selected_iteration_id: None,
            iterations: Vec::new(),
            file_roots: Vec::new(),
            viewed_files: Vec::new(),
            file_filter: String::new(),
            diff: DiffState::default(),
            context_lines: 3,
            inspector_tab: InspectorTab::Comments,
            notes: String::new(),
            notes_save_state: NotesSaveState::Saved,
            draft_comment: String::new(),
            draft_suggestion: String::new(),
            selected_code_fragment: String::new(),
            focused_suggestion_seed: String::new(),
            comments: Vec::new(),
            remote_threads: Vec::new(),
            show_only_current_file_comments: false,
            hide_bot_comments: false,
            is_markdown_formatted: true,
            markdown_blocks: Vec::new(),
            description_markdown_blocks: Vec::new(),
            is_busy: false,
            error_message: String::new(),
            status_message: String::new(),
            is_viewing_definition: false,
            definition_navigation: None,
            saved_diff: None,
        }
    }
}

impl fmt::Debug for ReviewSessionAppState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReviewSessionAppState")
            .field(
                "session_key",
                &self
                    .session
                    .as_ref()
                    .map(|state| state.details.reference.session_key.as_str()),
            )
            .field("selected_file_path", &self.selected_file_path)
            .field("selected_iteration_id", &self.selected_iteration_id)
            .field("diff_line_count", &self.diff.lines.len())
            .field("comment_count", &self.comments.len())
            .field("remote_thread_count", &self.remote_threads.len())
            .field("is_busy", &self.is_busy)
            .field("is_viewing_definition", &self.is_viewing_definition)
            .finish_non_exhaustive()
    }
}

impl ReviewSessionAppState {
    #[must_use]
    pub fn from_session_state(state: &ReviewSessionState) -> Self {
        let mut value = Self::default();
        value.replace_session_state(state);
        value
    }

    pub fn replace_session_state(&mut self, state: &ReviewSessionState) {
        self.session = Some(state.clone());
        self.notes.clone_from(&state.notes);
        self.viewed_files = state.viewed_files.clone();
        self.notes_save_state = NotesSaveState::Saved;
        self.error_message.clear();
        self.status_message.clear();
        self.diff.clear();
        self.markdown_blocks.clear();
        self.description_markdown_blocks = markdown::parse(&state.details.description);
        self.is_viewing_definition = false;
        self.definition_navigation = None;
        self.saved_diff = None;
        self.rebuild_iterations();
        self.rebuild_file_tree();
        self.rebuild_comments();
        let _ = self.ensure_initial_file_selected();
    }

    pub fn replace_session_state_preserving_view(&mut self, state: &ReviewSessionState) {
        let notes = self.notes.clone();
        let notes_save_state = self.notes_save_state;
        let draft_comment = self.draft_comment.clone();
        let draft_suggestion = self.draft_suggestion.clone();
        let selected_code_fragment = self.selected_code_fragment.clone();
        let focused_suggestion_seed = self.focused_suggestion_seed.clone();
        let viewed_files = self.viewed_files.clone();
        let selected_file_path = self.selected_file_path.clone();
        let selected_path_is_folder = self.selected_path_is_folder;
        let diff = self.diff.clone();
        let context_lines = self.context_lines;
        let inspector_tab = self.inspector_tab;
        let show_only_current_file_comments = self.show_only_current_file_comments;
        let hide_bot_comments = self.hide_bot_comments;
        let is_markdown_formatted = self.is_markdown_formatted;
        let markdown_blocks = self.markdown_blocks.clone();
        let is_viewing_definition = self.is_viewing_definition;
        let definition_navigation = self.definition_navigation.clone();
        let saved_diff = self.saved_diff.clone();

        self.replace_session_state(state);
        self.notes = notes;
        self.notes_save_state = notes_save_state;
        self.draft_comment = draft_comment;
        self.draft_suggestion = draft_suggestion;
        self.selected_code_fragment = selected_code_fragment;
        self.focused_suggestion_seed = focused_suggestion_seed;
        self.viewed_files = viewed_files;
        self.selected_file_path = selected_file_path;
        self.selected_path_is_folder = selected_path_is_folder;
        self.diff = diff;
        self.context_lines = context_lines;
        self.inspector_tab = inspector_tab;
        self.show_only_current_file_comments = show_only_current_file_comments;
        self.hide_bot_comments = hide_bot_comments;
        self.is_markdown_formatted = is_markdown_formatted;
        self.markdown_blocks = markdown_blocks;
        self.is_viewing_definition = is_viewing_definition;
        self.definition_navigation = definition_navigation;
        self.saved_diff = saved_diff;
        if let Some(session) = &mut self.session {
            session.notes.clone_from(&self.notes);
            session.viewed_files.clone_from(&self.viewed_files);
        }
        self.refresh_inline_comments();
    }

    pub fn replace_summary_state_preserving_view(&mut self, state: &ReviewSessionState) {
        let comments = self.comments.clone();
        let remote_threads = self.remote_threads.clone();
        self.replace_session_state_preserving_view(state);
        self.comments = comments;
        self.remote_threads = remote_threads;
        self.sync_comments_to_session();
        self.refresh_inline_comments();
    }

    pub fn replace_session_state_preserving_local_edits(&mut self, state: &ReviewSessionState) {
        let comments = self.comments.clone();
        self.replace_session_state_preserving_view(state);
        self.diff.clear();
        self.markdown_blocks.clear();
        self.is_viewing_definition = false;
        self.definition_navigation = None;
        self.saved_diff = None;
        if !self.selected_file_path.is_empty()
            && FileTreeNodeState::find(&self.file_roots, &self.selected_file_path).is_none()
        {
            self.selected_file_path.clear();
            self.selected_path_is_folder = false;
            let _ = self.ensure_initial_file_selected();
        }
        self.comments = comments;
        self.sync_comments_to_session();
        self.refresh_inline_comments();
    }

    #[must_use]
    pub fn has_active_review(&self) -> bool {
        self.session.is_some()
    }

    #[must_use]
    pub fn has_error(&self) -> bool {
        !self.error_message.trim().is_empty()
    }

    #[must_use]
    pub fn title(&self) -> &str {
        self.session
            .as_ref()
            .map_or("", |state| state.details.title.as_str())
    }

    #[must_use]
    pub fn author(&self) -> &str {
        self.session
            .as_ref()
            .map_or("", |state| state.details.author.as_str())
    }

    #[must_use]
    pub fn navigation_primary_title(&self) -> String {
        let Some(state) = &self.session else {
            return String::new();
        };
        format!(
            "#{} · {}",
            state.details.reference.number, state.details.title
        )
    }

    #[must_use]
    pub fn navigation_secondary_title(&self) -> String {
        let Some(state) = &self.session else {
            return String::new();
        };
        format!(
            "{} · {}",
            state.details.reference.host, state.details.reference.repository
        )
    }

    #[must_use]
    pub fn navigation_title(&self) -> String {
        let Some(state) = &self.session else {
            return String::new();
        };
        format!(
            "{} #{}",
            state.details.reference.repository, state.details.reference.number
        )
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.session.as_ref().is_some_and(|state| {
            !state.is_archived
                && state.completed_at_utc.is_none()
                && !is_provider_terminal(&state.details)
        })
    }

    #[must_use]
    pub fn is_past_review(&self) -> bool {
        self.has_active_review() && !self.is_open()
    }

    #[must_use]
    pub fn has_new_iteration(&self) -> bool {
        let Some(state) = &self.session else {
            return false;
        };
        state
            .last_reviewed_iteration_id
            .as_ref()
            .is_some_and(|reviewed| {
                state
                    .iterations
                    .last()
                    .is_some_and(|latest| latest.id != *reviewed)
            })
            && self.is_open()
    }

    #[must_use]
    pub fn persisted_snapshot(&self) -> Option<ReviewSessionState> {
        let mut state = self.session.clone()?;
        state.notes.clone_from(&self.notes);
        state.viewed_files.clone_from(&self.viewed_files);
        if !self.selected_file_path.is_empty() && !self.selected_path_is_folder {
            state.selected_file_path = Some(self.selected_file_path.clone());
        }
        state
            .selected_iteration_id
            .clone_from(&self.selected_iteration_id);
        state.comments = self
            .comments
            .iter()
            .map(|comment| comment.comment.clone())
            .collect();
        Some(state)
    }

    pub fn set_busy(&mut self, busy: bool) {
        self.is_busy = busy;
    }

    pub fn set_error(&mut self, error: impl Into<String>) {
        self.error_message = error.into();
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status_message = status.into();
    }

    pub fn clear_error(&mut self) {
        self.error_message.clear();
    }

    #[must_use]
    pub fn readiness(&self) -> ReviewActionReadiness {
        let has_session = self.has_active_review();
        let can_move_change = self.diff.change_count() > 0 && !self.is_busy;
        ReviewActionReadiness {
            can_refresh: has_session && !self.is_busy,
            can_mark_reviewed: has_session
                && !self.is_busy
                && self
                    .session
                    .as_ref()
                    .is_some_and(|state| !state.iterations.is_empty()),
            can_save_notes: has_session && !self.is_busy,
            can_save_comment: self.can_save_comment(),
            can_publish_comments: self.can_publish_pending_comments(),
            can_search_file: self.diff.can_search(),
            can_move_search_previous: self.diff.has_search_matches(),
            can_move_search_next: self.diff.has_search_matches(),
            can_move_change_previous: can_move_change,
            can_move_change_next: can_move_change,
            can_open_comment: self.can_open_comment(),
        }
    }

    #[must_use]
    pub fn pending_comment_count(&self) -> usize {
        self.comments
            .iter()
            .filter(|comment| comment.is_pending())
            .count()
    }

    #[must_use]
    pub fn published_comment_count(&self) -> usize {
        self.comments
            .iter()
            .filter(|comment| comment.is_published())
            .count()
    }

    #[must_use]
    pub fn has_pending_comments(&self) -> bool {
        self.pending_comment_count() > 0
    }

    #[must_use]
    pub fn can_publish_pending_comments(&self) -> bool {
        ReviewActionState::can_publish_comments(self.pending_comment_count(), self.is_busy)
    }

    #[must_use]
    pub fn can_open_comment(&self) -> bool {
        self.has_active_review()
            && !self.is_busy
            && !self.is_viewing_definition
            && self
                .diff
                .selected_line()
                .is_some_and(DiffLineState::is_commentable)
    }

    #[must_use]
    pub fn can_save_comment(&self) -> bool {
        let suggestion = if self.draft_comment.trim().is_empty()
            && self.draft_suggestion == self.focused_suggestion_seed
        {
            ""
        } else {
            self.draft_suggestion.as_str()
        };
        ReviewActionState::can_save_comment(
            self.has_active_review(),
            self.diff
                .selected_line()
                .is_some_and(DiffLineState::is_commentable)
                && !self.is_viewing_definition,
            &self.draft_comment,
            suggestion,
            self.is_busy,
        )
    }

    pub fn set_notes(&mut self, notes: impl Into<String>) {
        self.notes = notes.into();
        self.notes_save_state = NotesSaveState::Saving;
        if let Some(state) = &mut self.session {
            state.notes.clone_from(&self.notes);
        }
    }

    pub fn mark_notes_saved(&mut self) {
        self.notes_save_state = NotesSaveState::Saved;
    }

    pub fn mark_notes_save_failed(&mut self, error: impl Into<String>) {
        self.notes_save_state = NotesSaveState::Failed;
        self.error_message = error.into();
    }

    #[must_use]
    pub fn selected_iteration(&self) -> Option<&PullRequestIteration> {
        self.iterations.iter().find(|iteration| {
            self.selected_iteration_id
                .as_deref()
                .map_or(iteration.is_all_changes, |selected| {
                    iteration.id.eq_ignore_ascii_case(selected)
                })
        })
    }

    #[must_use]
    pub fn active_range(&self) -> Option<ReviewRangeState> {
        let session = self.session.as_ref()?;
        let selected = self.selected_iteration();
        Some(match selected {
            Some(iteration) if !iteration.is_all_changes => ReviewRangeState {
                iteration_id: Some(iteration.id.clone()),
                base_commit_sha: iteration.base_commit_sha.clone(),
                head_commit_sha: iteration.head_commit_sha.clone(),
            },
            _ => ReviewRangeState {
                iteration_id: None,
                base_commit_sha: session.details.base_commit_sha.clone(),
                head_commit_sha: session.details.head_commit_sha.clone(),
            },
        })
    }

    pub fn select_iteration(&mut self, iteration_id: Option<&str>) -> bool {
        let Some(iteration) = self.iterations.iter().find(|iteration| match iteration_id {
            Some(id) => iteration.id.eq_ignore_ascii_case(id),
            None => iteration.is_all_changes,
        }) else {
            return false;
        };
        let selected_iteration_id = (!iteration.is_all_changes).then(|| iteration.id.clone());
        if self.selected_iteration_id == selected_iteration_id {
            return false;
        }
        self.selected_iteration_id = selected_iteration_id;
        if let Some(state) = &mut self.session {
            state
                .selected_iteration_id
                .clone_from(&self.selected_iteration_id);
        }
        self.selected_file_path.clear();
        self.selected_path_is_folder = false;
        self.diff.clear();
        self.markdown_blocks.clear();
        self.is_viewing_definition = false;
        self.definition_navigation = None;
        self.saved_diff = None;
        self.rebuild_file_tree();
        true
    }

    pub fn set_file_filter(&mut self, filter: impl Into<String>) {
        self.file_filter = filter.into();
        self.rebuild_file_tree();
    }

    #[must_use]
    pub fn selected_tree_node(&self) -> Option<&FileTreeNodeState> {
        FileTreeNodeState::find(&self.file_roots, &self.selected_file_path)
    }

    #[must_use]
    pub fn selected_tree_node_mut(&mut self) -> Option<&mut FileTreeNodeState> {
        FileTreeNodeState::find_mut(&mut self.file_roots, &self.selected_file_path)
    }

    pub fn select_file_or_folder(&mut self, path: &str) -> bool {
        let Some(node) = FileTreeNodeState::find(&self.file_roots, path) else {
            return false;
        };
        let normalized = node.path.clone();
        self.selected_path_is_folder = node.is_folder;
        self.selected_file_path = normalized.clone();
        if node.is_folder {
            if let Some(node) = FileTreeNodeState::find_mut(&mut self.file_roots, &normalized) {
                node.is_expanded = true;
            }
        } else {
            mark_viewed_path(&mut self.viewed_files, &normalized);
            if let Some(node) = FileTreeNodeState::find_mut(&mut self.file_roots, &normalized) {
                node.is_viewed = true;
            }
            if let Some(state) = &mut self.session {
                state.selected_file_path = Some(normalized);
                state.viewed_files.clone_from(&self.viewed_files);
            }
        }
        FileTreeNodeState::expand_ancestors(&mut self.file_roots, &self.selected_file_path);
        self.diff.clear();
        self.diff
            .set_marker_eligibility(self.selected_file_has_diff_markers());
        self.markdown_blocks.clear();
        self.is_viewing_definition = false;
        self.definition_navigation = None;
        self.saved_diff = None;
        self.clear_comment_selection();
        true
    }

    #[must_use]
    pub fn ensure_initial_file_selected(&mut self) -> bool {
        if self.session.is_none()
            || !self.selected_file_path.is_empty()
            || self
                .session
                .as_ref()
                .is_none_or(|state| state.changed_files.is_empty())
        {
            return false;
        }

        let selected = self
            .session
            .as_ref()
            .and_then(|state| state.selected_file_path.as_deref())
            .filter(|path| {
                FileTreeNodeState::find(&self.file_roots, path)
                    .is_some_and(FileTreeNodeState::is_file)
            })
            .map(str::to_string)
            .or_else(|| {
                self.viewed_files.iter().rev().find_map(|path| {
                    FileTreeNodeState::find(&self.file_roots, path)
                        .filter(|node| node.is_file())
                        .map(|node| node.path.clone())
                })
            })
            .or_else(|| {
                FileTreeNodeState::find_first_file(&self.file_roots).map(|node| node.path.clone())
            });
        selected
            .as_deref()
            .is_some_and(|path| self.select_file_or_folder(path))
    }

    #[must_use]
    pub fn changed_file_count(&self) -> usize {
        self.session
            .as_ref()
            .map_or(0, |state| state.changed_files.len())
    }

    #[must_use]
    pub fn is_markdown_file(&self) -> bool {
        !self.selected_path_is_folder
            && self
                .selected_file_path
                .rsplit_once('.')
                .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("md"))
    }

    #[must_use]
    pub fn is_formatted_markdown_visible(&self) -> bool {
        self.is_markdown_file() && self.is_markdown_formatted
    }

    #[must_use]
    pub fn is_raw_code_visible(&self) -> bool {
        !self.is_formatted_markdown_visible()
    }

    pub fn toggle_markdown_format(&mut self) -> bool {
        if !self.is_markdown_file() {
            return false;
        }
        self.is_markdown_formatted = !self.is_markdown_formatted;
        true
    }

    pub fn set_markdown_content(&mut self, content: &str) -> bool {
        if !self.is_markdown_file() {
            self.markdown_blocks.clear();
            return false;
        }
        self.markdown_blocks = markdown::parse(content);
        true
    }

    #[must_use]
    pub fn has_description(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|state| !state.details.description.trim().is_empty())
    }

    pub fn replace_diff_blocks(&mut self, blocks: &[DiffBlock]) {
        self.diff.allow_folds = true;
        self.diff
            .replace_blocks(blocks, self.selected_path_is_folder);
        self.diff
            .set_marker_eligibility(self.selected_file_has_diff_markers());
        self.refresh_inline_comments();
    }

    pub fn replace_diff_lines(&mut self, lines: impl IntoIterator<Item = DiffLine>) {
        self.diff.allow_folds = true;
        self.diff.replace_lines(lines, self.selected_path_is_folder);
        self.diff
            .set_marker_eligibility(self.selected_file_has_diff_markers());
        self.refresh_inline_comments();
    }

    pub fn set_diff_layout(&mut self, layout: DiffLayout) {
        self.diff.set_layout(layout);
    }

    pub fn set_focused_comment_mode(&mut self, enabled: bool) {
        self.diff.set_focused_comment_mode(enabled);
    }

    pub fn set_context_lines(&mut self, context_lines: u32) {
        self.context_lines = context_lines;
    }

    #[must_use]
    pub fn selected_diff_line(&self) -> Option<&DiffLineState> {
        self.diff.selected_line()
    }

    pub fn select_diff_line(&mut self, source_index: usize) -> bool {
        let changed = self
            .diff
            .selected_line_index
            .is_none_or(|selected| selected != source_index);
        if !self.diff.select_line(source_index) {
            return false;
        }
        if changed {
            self.clear_comment_selection();
        }
        true
    }

    pub fn select_code_range(
        &mut self,
        source_index: usize,
        start: usize,
        length: usize,
    ) -> Option<String> {
        if !self.select_diff_line(source_index) {
            return None;
        }
        let selected = self
            .diff
            .selected_line_mut()?
            .select_code_range(start, length);
        self.selected_code_fragment.clone_from(&selected);
        Some(selected)
    }

    pub fn prepare_focused_comment(&mut self) {
        if !self.draft_suggestion.trim().is_empty() {
            return;
        }
        self.focused_suggestion_seed = if self.selected_code_fragment.trim().is_empty() {
            self.selected_diff_line()
                .map_or_else(String::new, |line| line.line.text.clone())
        } else {
            self.selected_code_fragment.clone()
        };
        self.draft_suggestion
            .clone_from(&self.focused_suggestion_seed);
    }

    pub fn cancel_comment_draft(&mut self) {
        self.draft_comment.clear();
        self.clear_comment_selection();
    }

    pub fn queue_comment(&mut self) -> Option<String> {
        if !self.can_save_comment() {
            return None;
        }
        let line = self.selected_diff_line()?.clone();
        let side = if line.line.kind == DiffLineKind::Deleted {
            ReviewCommentSide::Left
        } else {
            line.comment_side()
        };
        let line_number = line.line_number_for_side(side)?;
        let body = self.draft_comment.trim();
        let suggestion = self.draft_suggestion.trim_end();
        let body = if body.is_empty() {
            "Suggested change.".to_string()
        } else {
            body.to_string()
        };
        let range = self.active_range()?;
        let mut comment = ReviewComment {
            file_path: line.line.file_path,
            line_number,
            side,
            body,
            source_line_text: line.line.text,
            selection_start: (line.selection_length > 0).then_some(line.selection_start),
            selection_length: line.selection_length,
            selected_text: self.selected_code_fragment.clone(),
            suggested_replacement: suggestion.to_string(),
            iteration_id: range.iteration_id,
            commit_sha: Some(range.head_commit_sha),
            ..Default::default()
        };
        if comment.id.is_empty() {
            comment.id = format!(
                "local:{}:{}:{}",
                comment.file_path,
                comment.line_number,
                self.comments.len()
            );
        }
        let id = comment.id.clone();
        self.comments.push(LocalReviewCommentState::new(comment));
        self.comments.sort_by(|left, right| {
            right
                .comment
                .created_at_utc
                .cmp(&left.comment.created_at_utc)
        });
        self.draft_comment.clear();
        self.clear_comment_selection();
        self.sync_comments_to_session();
        self.refresh_inline_comments();
        Some(id)
    }

    pub fn begin_comment_edit(&mut self, comment_id: &str) -> bool {
        self.comments
            .iter_mut()
            .find(|comment| comment.id().eq_ignore_ascii_case(comment_id))
            .is_some_and(LocalReviewCommentState::begin_edit)
    }

    pub fn cancel_comment_edit(&mut self, comment_id: &str) -> bool {
        let Some(comment) = self
            .comments
            .iter_mut()
            .find(|comment| comment.id().eq_ignore_ascii_case(comment_id))
        else {
            return false;
        };
        comment.cancel_edit();
        true
    }

    pub fn commit_comment_edit(&mut self, comment_id: &str) -> bool {
        if self.is_busy {
            return false;
        }
        let committed = self
            .comments
            .iter_mut()
            .find(|comment| comment.id().eq_ignore_ascii_case(comment_id))
            .is_some_and(LocalReviewCommentState::commit_edit);
        if committed {
            self.sync_comments_to_session();
        }
        committed
    }

    #[must_use]
    pub fn can_commit_comment_edit(&self, comment_id: &str) -> bool {
        !self.is_busy
            && self
                .comments
                .iter()
                .find(|comment| comment.id().eq_ignore_ascii_case(comment_id))
                .is_some_and(LocalReviewCommentState::can_save_edit)
    }

    pub fn remove_comment(&mut self, comment_id: &str) -> bool {
        let Some(index) = self
            .comments
            .iter()
            .position(|comment| comment.id().eq_ignore_ascii_case(comment_id))
        else {
            return false;
        };
        self.comments.remove(index);
        self.sync_comments_to_session();
        self.refresh_inline_comments();
        true
    }

    pub fn mark_comment_published(
        &mut self,
        comment_id: &str,
        remote_id: impl Into<String>,
    ) -> bool {
        let remote_id = remote_id.into();
        let Some(comment) = self
            .comments
            .iter_mut()
            .find(|comment| comment.id().eq_ignore_ascii_case(comment_id))
        else {
            return false;
        };
        comment.comment.status = ReviewCommentStatus::Published;
        comment.comment.remote_id = Some(remote_id);
        comment.comment.error = None;
        comment.comment.published_at_utc = Some(chrono::Utc::now());
        self.sync_comments_to_session();
        true
    }

    pub fn mark_comment_failed(&mut self, comment_id: &str, error: impl Into<String>) -> bool {
        let error = error.into();
        let Some(comment) = self
            .comments
            .iter_mut()
            .find(|comment| comment.id().eq_ignore_ascii_case(comment_id))
        else {
            return false;
        };
        comment.comment.status = ReviewCommentStatus::Failed;
        comment.comment.error = (!error.trim().is_empty()).then_some(error);
        self.sync_comments_to_session();
        true
    }

    #[must_use]
    pub fn visible_comments(&self) -> Vec<&LocalReviewCommentState> {
        self.comments
            .iter()
            .filter(|comment| {
                !self.show_only_current_file_comments
                    || comment.matches_file(&self.selected_file_path)
            })
            .collect()
    }

    #[must_use]
    pub fn visible_remote_threads(&self) -> Vec<&RemoteReviewThreadState> {
        self.remote_threads
            .iter()
            .filter(|thread| {
                (!self.hide_bot_comments || !thread.is_bot())
                    && (!self.show_only_current_file_comments
                        || thread.matches_file(&self.selected_file_path))
            })
            .collect()
    }

    pub fn set_remote_comments(&mut self, comments: &[RemoteReviewComment]) {
        if let Some(state) = &mut self.session {
            state.remote_comments = comments.to_vec();
        }
        self.rebuild_comments();
    }

    pub fn begin_remote_reply(&mut self, thread_id: &str) -> bool {
        let Some(thread) = self
            .remote_threads
            .iter_mut()
            .find(|thread| thread.id.eq_ignore_ascii_case(thread_id))
        else {
            return false;
        };
        thread.begin_reply();
        true
    }

    pub fn cancel_remote_reply(&mut self, thread_id: &str) -> bool {
        let Some(thread) = self
            .remote_threads
            .iter_mut()
            .find(|thread| thread.id.eq_ignore_ascii_case(thread_id))
        else {
            return false;
        };
        thread.cancel_reply();
        true
    }

    #[must_use]
    pub fn remote_reply_draft(&self, thread_id: &str) -> Option<&str> {
        self.remote_threads
            .iter()
            .find(|thread| thread.id.eq_ignore_ascii_case(thread_id))
            .and_then(RemoteReviewThreadState::reply_draft)
    }

    #[must_use]
    pub fn can_submit_remote_reply(&self, thread_id: &str) -> bool {
        !self.is_busy
            && self
                .remote_threads
                .iter()
                .find(|thread| thread.id.eq_ignore_ascii_case(thread_id))
                .is_some_and(RemoteReviewThreadState::can_submit_reply)
    }

    pub fn complete_remote_reply(&mut self, thread_id: &str) -> bool {
        let Some(thread) = self
            .remote_threads
            .iter_mut()
            .find(|thread| thread.id.eq_ignore_ascii_case(thread_id))
        else {
            return false;
        };
        thread.complete_reply();
        true
    }

    pub fn navigate_to_change(&mut self, direction: isize) -> Option<usize> {
        if self.is_busy {
            return None;
        }
        let line_index = self.diff.navigate_change(direction)?;
        self.selected_code_fragment.clear();
        self.draft_suggestion.clear();
        self.focused_suggestion_seed.clear();
        Some(line_index)
    }

    pub fn set_search_query(&mut self, query: impl Into<String>) {
        self.diff.set_search_query(query);
    }

    pub fn navigate_search(&mut self, direction: isize) -> Option<usize> {
        self.diff.navigate_search(direction)
    }

    pub fn show_definition(
        &mut self,
        symbol: impl Into<String>,
        location: SymbolLocation,
        content: &str,
    ) -> bool {
        if self.session.is_none() || location.line_number == 0 {
            return false;
        }
        let normalized = content.replace("\r\n", "\n");
        let lines = normalized.split('\n').collect::<Vec<_>>();
        let target_index = location.line_number.saturating_sub(1);
        if target_index >= lines.len() {
            return false;
        }
        if !self.is_viewing_definition {
            self.saved_diff = Some(self.diff.clone());
        }
        let start = target_index.saturating_sub(100);
        let end = lines.len().min(target_index.saturating_add(201));
        let mut definition_lines = vec![DiffLine {
            kind: DiffLineKind::Hunk,
            file_path: location.file_path.clone(),
            text: format!(
                "Definition: {}:{}",
                location.file_path, location.line_number
            ),
            ..Default::default()
        }];
        for (index, text) in lines.iter().enumerate().take(end).skip(start) {
            definition_lines.push(DiffLine {
                kind: DiffLineKind::Context,
                file_path: location.file_path.clone(),
                new_line_number: Some(index + 1),
                text: (*text).to_string(),
                ..Default::default()
            });
        }
        let symbol = symbol.into();
        self.diff.allow_folds = false;
        self.diff.replace_lines(definition_lines, false);
        let selected = self
            .diff
            .lines
            .iter()
            .position(|line| line.line.new_line_number == Some(location.line_number));
        if let Some(selected) = selected {
            self.diff.select_line(selected);
        }
        self.definition_navigation = Some(DefinitionNavigationState {
            symbol: symbol.clone(),
            heading: format!(
                "{}:{} - {}",
                location.file_path, location.line_number, symbol
            ),
            location,
        });
        self.is_viewing_definition = true;
        true
    }

    pub fn return_to_diff(&mut self) -> bool {
        if !self.is_viewing_definition {
            return false;
        }
        if let Some(saved) = self.saved_diff.take() {
            self.diff = saved;
        }
        self.is_viewing_definition = false;
        self.definition_navigation = None;
        true
    }

    pub fn collapse_all_functions(&mut self) -> usize {
        self.diff.collapse_all_folds()
    }

    pub fn expand_all_functions(&mut self) {
        self.diff.expand_all_folds();
    }

    pub fn collapse_all_comments(&mut self) {
        for comment in &mut self.comments {
            comment.is_expanded = false;
        }
        for thread in &mut self.remote_threads {
            thread.is_expanded = false;
        }
    }

    pub fn expand_all_comments(&mut self) {
        for comment in &mut self.comments {
            comment.is_expanded = true;
        }
        for thread in &mut self.remote_threads {
            thread.is_expanded = true;
        }
    }

    fn clear_comment_selection(&mut self) {
        self.selected_code_fragment.clear();
        self.draft_suggestion.clear();
        self.focused_suggestion_seed.clear();
    }

    fn rebuild_iterations(&mut self) {
        let Some(state) = &self.session else {
            self.iterations.clear();
            self.selected_iteration_id = None;
            return;
        };
        self.iterations.clear();
        self.iterations.push(PullRequestIteration {
            id: PullRequestIteration::ALL_CHANGES_ID.to_string(),
            number: 0,
            label: "All changes".to_string(),
            base_commit_sha: state.details.base_commit_sha.clone(),
            head_commit_sha: state.details.head_commit_sha.clone(),
            is_all_changes: true,
            ..Default::default()
        });
        self.iterations.extend(state.iterations.iter().cloned());
        self.selected_iteration_id = state.selected_iteration_id.clone().filter(|selected| {
            self.iterations.iter().any(|iteration| {
                !iteration.is_all_changes && iteration.id.eq_ignore_ascii_case(selected)
            })
        });
    }

    fn rebuild_file_tree(&mut self) {
        let Some(state) = &self.session else {
            self.file_roots.clear();
            return;
        };
        let filter = self.file_filter.trim();
        let files = state
            .changed_files
            .iter()
            .filter(|file| {
                filter.is_empty()
                    || file
                        .path
                        .to_ascii_lowercase()
                        .contains(&filter.to_ascii_lowercase())
            })
            .cloned()
            .collect::<Vec<_>>();
        self.file_roots = FileTreeBuilder::build(&files, &self.viewed_files);
    }

    fn rebuild_comments(&mut self) {
        let Some(state) = &self.session else {
            self.comments.clear();
            self.remote_threads.clear();
            return;
        };
        self.comments = state
            .comments
            .iter()
            .cloned()
            .map(LocalReviewCommentState::new)
            .collect();
        self.comments.sort_by(|left, right| {
            right
                .comment
                .created_at_utc
                .cmp(&left.comment.created_at_utc)
        });
        let published_ids = self
            .comments
            .iter()
            .filter_map(|comment| comment.comment.remote_id.as_deref())
            .filter(|id| !id.trim().is_empty())
            .collect::<HashSet<_>>();
        let remote = state
            .remote_comments
            .iter()
            .filter(|comment| {
                comment.remote_id.trim().is_empty()
                    || !published_ids.contains(comment.remote_id.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        self.remote_threads = build_remote_comment_threads(&remote, true);
        self.refresh_inline_comments();
    }

    fn sync_comments_to_session(&mut self) {
        if let Some(state) = &mut self.session {
            state.comments = self
                .comments
                .iter()
                .map(|comment| comment.comment.clone())
                .collect();
        }
    }

    fn refresh_inline_comments(&mut self) {
        let mut line_comments = BTreeMap::<usize, (Vec<String>, Vec<String>)>::new();
        for (line_index, line) in self.diff.lines.iter().enumerate() {
            let mut left_ids = Vec::new();
            let mut right_ids = Vec::new();
            for comment in &self.comments {
                if comment_matches_line(
                    &comment.comment.file_path,
                    comment.comment.line_number,
                    comment.comment.side,
                    &line.line,
                ) {
                    match comment.comment.side {
                        ReviewCommentSide::Left => {
                            left_ids.push(comment.comment.id.clone());
                        }
                        ReviewCommentSide::Right => {
                            right_ids.push(comment.comment.id.clone());
                        }
                    }
                }
            }
            for thread in &self.remote_threads {
                for remote in std::iter::once(&thread.root).chain(thread.replies.iter()) {
                    if let Some((_, side)) =
                        remote.comment.line_number.zip(remote.comment.side).filter(
                            |(line_number, side)| {
                                comment_matches_line(
                                    &remote.comment.file_path,
                                    *line_number,
                                    *side,
                                    &line.line,
                                )
                            },
                        )
                    {
                        match side {
                            ReviewCommentSide::Left => left_ids.push(thread.id.clone()),
                            ReviewCommentSide::Right => right_ids.push(thread.id.clone()),
                        }
                    }
                }
            }
            left_ids.sort();
            left_ids.dedup();
            right_ids.sort();
            right_ids.dedup();
            line_comments.insert(line_index, (left_ids, right_ids));
        }
        for (line_index, (left_ids, right_ids)) in line_comments {
            self.diff
                .set_line_comment_ids_by_side(line_index, left_ids, right_ids);
        }
        let show_change_markers = self.diff.show_change_markers;
        self.diff.rebuild_markers(show_change_markers);
        self.diff.rebuild_rows();
    }

    fn selected_file_has_diff_markers(&self) -> bool {
        if self.selected_path_is_folder || self.selected_file_path.trim().is_empty() {
            return false;
        }
        self.session.as_ref().is_some_and(|state| {
            state.changed_files.iter().any(|file| {
                paths_equal(&file.path, &self.selected_file_path)
                    && matches!(
                        file.status,
                        ChangedFileStatus::Modified
                            | ChangedFileStatus::Renamed
                            | ChangedFileStatus::Copied
                            | ChangedFileStatus::TypeChanged
                    )
            })
        })
    }
}

impl From<&ReviewSessionState> for ReviewSessionAppState {
    fn from(state: &ReviewSessionState) -> Self {
        Self::from_session_state(state)
    }
}

impl TryFrom<&[ReviewSessionState]> for ReviewSessionAppState {
    type Error = &'static str;

    fn try_from(states: &[ReviewSessionState]) -> Result<Self, Self::Error> {
        let state = states.first().ok_or("a review session state is required")?;
        Ok(Self::from(state))
    }
}

fn mark_viewed_path(paths: &mut Vec<String>, path: &str) {
    if !paths.iter().any(|existing| paths_equal(existing, path)) {
        paths.push(normalized_path(path));
    }
}

fn comment_matches_line(
    file_path: &str,
    line_number: usize,
    side: ReviewCommentSide,
    line: &DiffLine,
) -> bool {
    if !paths_equal(file_path, &line.file_path) {
        return false;
    }
    let actual = match side {
        ReviewCommentSide::Left => line.old_line_number,
        ReviewCommentSide::Right => line.new_line_number,
    };
    actual == Some(line_number)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActivityMonth {
    pub year: i32,
    pub month: u32,
}

impl Default for ActivityMonth {
    fn default() -> Self {
        let today = Local::now().date_naive();
        Self {
            year: today.year(),
            month: today.month(),
        }
    }
}

impl ActivityMonth {
    #[must_use]
    pub const fn new(year: i32, month: u32) -> Option<Self> {
        if year >= 1 && year <= 9999 && month >= 1 && month <= 12 {
            Some(Self { year, month })
        } else {
            None
        }
    }

    #[must_use]
    pub fn previous(self) -> Self {
        if self.month == 1 {
            Self {
                year: self.year - 1,
                month: 12,
            }
        } else {
            Self {
                year: self.year,
                month: self.month - 1,
            }
        }
    }

    #[must_use]
    pub fn next(self) -> Self {
        if self.month == 12 {
            Self {
                year: self.year + 1,
                month: 1,
            }
        } else {
            Self {
                year: self.year,
                month: self.month + 1,
            }
        }
    }

    #[must_use]
    pub fn first_day(self) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(self.year, self.month, 1)
    }

    #[must_use]
    pub fn days_in_month(self) -> usize {
        self.next()
            .first_day()
            .zip(self.first_day())
            .map_or(0, |(next, current)| (next - current).num_days() as usize)
    }

    #[must_use]
    pub fn label(self) -> String {
        self.first_day().map_or_else(
            || format!("{}-{:02}", self.year, self.month),
            |date| date.format("%B %Y").to_string(),
        )
    }
}

impl From<NaiveDate> for ActivityMonth {
    fn from(date: NaiveDate) -> Self {
        Self {
            year: date.year(),
            month: date.month(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewActivityDayState {
    pub date: NaiveDate,
    pub pull_request_count: i32,
    pub comment_count: i32,
    pub review_count: i32,
    pub pull_request_bar_height: f64,
    pub comment_bar_height: f64,
    pub review_bar_height: f64,
}

impl ReviewActivityDayState {
    #[must_use]
    pub fn day_label(&self) -> String {
        self.date.day().to_string()
    }
}

impl From<&ReviewActivityDay> for ReviewActivityDayState {
    fn from(day: &ReviewActivityDay) -> Self {
        Self {
            date: day.date,
            pull_request_count: day.pull_request_count,
            comment_count: day.comment_count,
            review_count: day.review_count,
            pull_request_bar_height: 0.0,
            comment_bar_height: 0.0,
            review_bar_height: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentReviewActivityStatus {
    Completed,
    MarkedReviewed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentReviewActivityState {
    pub session_key: String,
    pub provider: ReviewProvider,
    pub repository: String,
    pub pull_request_number: u32,
    pub pull_request_title: String,
    pub status: RecentReviewActivityStatus,
    pub date: NaiveDate,
}

impl RecentReviewActivityState {
    #[must_use]
    pub fn location(&self) -> String {
        format!("{} #{}", self.repository, self.pull_request_number)
    }
}

impl From<&ReviewActivityRecord> for RecentReviewActivityState {
    fn from(record: &ReviewActivityRecord) -> Self {
        Self {
            session_key: record.session_key.clone(),
            provider: record.provider,
            repository: record.repository.clone(),
            pull_request_number: record.pull_request_number,
            pull_request_title: record.pull_request_title.clone(),
            status: if record.kind == ReviewActivityKind::Completed {
                RecentReviewActivityStatus::Completed
            } else {
                RecentReviewActivityStatus::MarkedReviewed
            },
            date: record.at_utc.with_timezone(&Local).date_naive(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct ReviewActivityPageState {
    pub visible_month: ActivityMonth,
    pub latest_month: ActivityMonth,
    pub days: Vec<ReviewActivityDayState>,
    pub recent_reviews: Vec<RecentReviewActivityState>,
    pub pull_request_total: i32,
    pub comment_total: i32,
    pub review_total: i32,
    pub chart_maximum: i32,
    pub is_busy: bool,
    pub error_message: String,
    pub recent_page_index: usize,
    pub recent_page_size: usize,
}

impl Default for ReviewActivityPageState {
    fn default() -> Self {
        let month = ActivityMonth::default();
        Self {
            visible_month: month,
            latest_month: month,
            days: empty_month_days(month),
            recent_reviews: Vec::new(),
            pull_request_total: 0,
            comment_total: 0,
            review_total: 0,
            chart_maximum: 5,
            is_busy: false,
            error_message: String::new(),
            recent_page_index: 0,
            recent_page_size: 10,
        }
    }
}

impl fmt::Debug for ReviewActivityPageState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReviewActivityPageState")
            .field("visible_month", &self.visible_month)
            .field("day_count", &self.days.len())
            .field("pull_request_total", &self.pull_request_total)
            .field("comment_total", &self.comment_total)
            .field("review_total", &self.review_total)
            .field("recent_review_count", &self.recent_reviews.len())
            .field("is_busy", &self.is_busy)
            .finish_non_exhaustive()
    }
}

impl ReviewActivityPageState {
    #[must_use]
    pub fn for_month(month: ActivityMonth) -> Self {
        Self {
            visible_month: month,
            latest_month: month,
            days: empty_month_days(month),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn month_label(&self) -> String {
        self.visible_month.label()
    }

    #[must_use]
    pub fn chart_midpoint(&self) -> i32 {
        self.chart_maximum / 2
    }

    #[must_use]
    pub fn can_move_to_next_month(&self) -> bool {
        self.visible_month < self.latest_month
    }

    #[must_use]
    pub fn can_move_to_previous_month(&self) -> bool {
        self.visible_month.previous().first_day().is_some()
    }

    #[must_use]
    pub fn can_page_recent_previous(&self) -> bool {
        self.recent_page_index > 0
    }

    #[must_use]
    pub fn can_page_recent_next(&self) -> bool {
        self.recent_page_index
            .saturating_add(1)
            .saturating_mul(self.recent_page_size)
            < self.recent_reviews.len()
    }

    #[must_use]
    pub fn recent_page(&self) -> &[RecentReviewActivityState] {
        let start = self
            .recent_page_index
            .saturating_mul(self.recent_page_size)
            .min(self.recent_reviews.len());
        let end = start
            .saturating_add(self.recent_page_size)
            .min(self.recent_reviews.len());
        &self.recent_reviews[start..end]
    }

    #[must_use]
    pub fn has_error(&self) -> bool {
        !self.error_message.trim().is_empty()
    }

    #[must_use]
    pub fn has_recent_reviews(&self) -> bool {
        !self.recent_reviews.is_empty()
    }

    pub fn previous_month(&mut self) -> ActivityMonth {
        self.visible_month = self.visible_month.previous();
        self.days = empty_month_days(self.visible_month);
        self.pull_request_total = 0;
        self.comment_total = 0;
        self.review_total = 0;
        self.chart_maximum = 5;
        self.visible_month
    }

    pub fn next_month(&mut self) -> Option<ActivityMonth> {
        if !self.can_move_to_next_month() {
            return None;
        }
        self.visible_month = self.visible_month.next();
        if self.visible_month > self.latest_month {
            self.visible_month = self.latest_month;
            return None;
        }
        self.days = empty_month_days(self.visible_month);
        self.pull_request_total = 0;
        self.comment_total = 0;
        self.review_total = 0;
        self.chart_maximum = 5;
        Some(self.visible_month)
    }

    pub fn page_recent_previous(&mut self) -> bool {
        if !self.can_page_recent_previous() {
            return false;
        }
        self.recent_page_index -= 1;
        true
    }

    pub fn page_recent_next(&mut self) -> bool {
        if !self.can_page_recent_next() {
            return false;
        }
        self.recent_page_index += 1;
        true
    }

    pub fn set_recent_page_size(&mut self, page_size: usize) {
        self.recent_page_size = page_size.max(1);
        self.recent_page_index = 0;
    }

    pub fn load_month(
        &mut self,
        month: ActivityMonth,
        days: &[ReviewActivityDay],
        recent_reviews: &[ReviewActivityRecord],
    ) {
        self.visible_month = month;
        self.project_daily_counts(days);
        self.project_recent_reviews(recent_reviews);
        self.is_busy = false;
        self.error_message.clear();
    }

    pub fn aggregate_records(&mut self, month: ActivityMonth, records: &[ReviewActivityRecord]) {
        let mut sessions = BTreeMap::<NaiveDate, BTreeSet<String>>::new();
        let mut comment_counts = BTreeMap::<NaiveDate, i32>::new();
        let mut review_counts = BTreeMap::<NaiveDate, i32>::new();
        for record in records {
            let date = record.at_utc.with_timezone(&Local).date_naive();
            if ActivityMonth::from(date) != month {
                continue;
            }
            match record.kind {
                ReviewActivityKind::Opened
                | ReviewActivityKind::Completed
                | ReviewActivityKind::MarkedReviewed => {
                    sessions
                        .entry(date)
                        .or_default()
                        .insert(record.session_key.clone());
                }
                ReviewActivityKind::CommentPublished | ReviewActivityKind::ReplyPublished => {}
            }
            if record.comment_delta != 0 {
                let total = comment_counts.entry(date).or_default();
                *total = total.saturating_add(record.comment_delta);
            }
            if record.kind == ReviewActivityKind::MarkedReviewed {
                let total = review_counts.entry(date).or_default();
                *total = total.saturating_add(1);
            }
        }

        let days = empty_dates(month)
            .into_iter()
            .map(|date| ReviewActivityDay {
                date,
                pull_request_count: sessions
                    .get(&date)
                    .map_or(0, |values| i32::try_from(values.len()).unwrap_or(i32::MAX)),
                comment_count: comment_counts.get(&date).copied().unwrap_or(0),
                review_count: review_counts.get(&date).copied().unwrap_or(0),
            })
            .collect::<Vec<_>>();
        self.load_month(month, &days, records);
    }

    fn project_daily_counts(&mut self, source_days: &[ReviewActivityDay]) {
        let mut aggregated = empty_dates(self.visible_month)
            .into_iter()
            .map(|date| (date, (0_i32, 0_i32, 0_i32)))
            .collect::<BTreeMap<_, _>>();
        for day in source_days {
            if ActivityMonth::from(day.date) != self.visible_month {
                continue;
            }
            let counts = aggregated.entry(day.date).or_default();
            counts.0 = counts.0.saturating_add(day.pull_request_count);
            counts.1 = counts.1.saturating_add(day.comment_count);
            counts.2 = counts.2.saturating_add(day.review_count);
        }
        self.days = aggregated
            .into_iter()
            .map(
                |(date, (pull_request_count, comment_count, review_count))| {
                    ReviewActivityDayState {
                        date,
                        pull_request_count,
                        comment_count,
                        review_count,
                        pull_request_bar_height: 0.0,
                        comment_bar_height: 0.0,
                        review_bar_height: 0.0,
                    }
                },
            )
            .collect();
        self.pull_request_total = self
            .days
            .iter()
            .fold(0, |total, day| total.saturating_add(day.pull_request_count));
        self.comment_total = self
            .days
            .iter()
            .fold(0, |total, day| total.saturating_add(day.comment_count));
        self.review_total = self
            .days
            .iter()
            .fold(0, |total, day| total.saturating_add(day.review_count));
        let maximum = self
            .days
            .iter()
            .flat_map(|day| [day.pull_request_count, day.comment_count, day.review_count])
            .max()
            .unwrap_or(0);
        self.chart_maximum = round_chart_maximum(maximum);
        for day in &mut self.days {
            day.pull_request_bar_height = scale_height(day.pull_request_count, self.chart_maximum);
            day.comment_bar_height = scale_height(day.comment_count, self.chart_maximum);
            day.review_bar_height = scale_height(day.review_count, self.chart_maximum);
        }
    }

    fn project_recent_reviews(&mut self, records: &[ReviewActivityRecord]) {
        let mut records = records
            .iter()
            .filter(|record| {
                matches!(
                    record.kind,
                    ReviewActivityKind::Completed | ReviewActivityKind::MarkedReviewed
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| std::cmp::Reverse(record.at_utc));
        let mut seen = HashSet::new();
        self.recent_reviews = records
            .into_iter()
            .filter(|record| seen.insert(record.session_key.clone()))
            .map(|record| RecentReviewActivityState::from(&record))
            .collect();
        self.recent_page_index = 0;
    }
}

#[must_use]
pub fn round_chart_maximum(maximum: i32) -> i32 {
    maximum
        .max(0)
        .saturating_add(4)
        .saturating_div(5)
        .saturating_mul(5)
        .max(5)
}

#[must_use]
pub fn scale_height(value: i32, maximum: i32) -> f64 {
    if value <= 0 {
        0.0
    } else {
        4.0_f64.max(f64::from(value) * 152.0 / f64::from(maximum.max(1)))
    }
}

fn empty_dates(month: ActivityMonth) -> Vec<NaiveDate> {
    let Some(first) = month.first_day() else {
        return Vec::new();
    };
    (0..month.days_in_month())
        .filter_map(|offset| first.checked_add_days(chrono::Days::new(offset as u64)))
        .collect()
}

fn empty_month_days(month: ActivityMonth) -> Vec<ReviewActivityDayState> {
    empty_dates(month)
        .into_iter()
        .map(|date| ReviewActivityDayState {
            date,
            pull_request_count: 0,
            comment_count: 0,
            review_count: 0,
            pull_request_bar_height: 0.0,
            comment_bar_height: 0.0,
            review_bar_height: 0.0,
        })
        .collect()
}

pub type AppState = MainAppState;
pub type MainViewState = MainAppState;
pub type ReviewState = ReviewSessionAppState;
pub type ReviewSessionViewState = ReviewSessionAppState;
pub type DiffViewState = DiffState;
pub type ReviewActivityState = ReviewActivityPageState;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn review_state(session_key: &str, opened_day: u32) -> ReviewSessionState {
        ReviewSessionState {
            details: PullRequestDetails {
                reference: prs_core::PullRequestReference {
                    provider: ReviewProvider::GitHub,
                    host: "github.example".to_string(),
                    repository: "octo-org/widget".to_string(),
                    number: opened_day,
                    session_key: session_key.to_string(),
                    web_url: "https://github.example/octo-org/widget/pull/1".to_string(),
                    ..Default::default()
                },
                title: format!("Review {session_key}"),
                base_commit_sha: "base".to_string(),
                head_commit_sha: "head".to_string(),
                ..Default::default()
            },
            last_opened_utc: Utc
                .with_ymd_and_hms(2026, 11, opened_day, 12, 0, 0)
                .unwrap(),
            ..Default::default()
        }
    }

    #[test]
    fn navigation_order_and_selection_remain_stable() {
        let mut older = review_state("older", 2);
        let mut newer = review_state("newer", 3);
        let equal = review_state("equal-to-newer", 3);
        let mut state = MainAppState::from_review_states([&older, &newer, &equal]);

        assert_eq!(
            state
                .active_reviews
                .iter()
                .map(|review| review.session_key.as_str())
                .collect::<Vec<_>>(),
            ["newer", "equal-to-newer", "older"]
        );
        assert!(state.select_review("older"));

        older.last_opened_utc = Utc.with_ymd_and_hms(2026, 11, 4, 12, 0, 0).unwrap();
        state.track_review(&older);
        assert_eq!(state.active_reviews[0].session_key, "older");
        assert_eq!(state.active_review_key.as_deref(), Some("older"));

        older.is_archived = true;
        state.track_review(&older);
        assert_eq!(state.active_review_key.as_deref(), Some("older"));
        assert!(
            state
                .reviewed_reviews
                .iter()
                .any(|review| review.session_key == "older")
        );
        newer.is_archived = true;
        state.track_review(&newer);
        assert!(
            state
                .selected_review()
                .is_some_and(|review| review.session_key == "older")
        );
    }

    #[test]
    fn file_tree_folds_single_child_folder_chains() {
        let file = ChangedFile {
            path: r"src\features\review\File.cs".to_string(),
            status: ChangedFileStatus::Modified,
            ..Default::default()
        };
        let roots = FileTreeBuilder::build(&[file], ["SRC/FEATURES/REVIEW/FILE.CS"]);

        assert_eq!(roots.len(), 1);
        let root = &roots[0];
        assert!(root.is_folder);
        assert_eq!(root.name, "src/features/review");
        assert_eq!(root.path, "src/features/review");
        assert_eq!(root.children.len(), 1);
        let leaf = &root.children[0];
        assert!(leaf.is_file());
        assert_eq!(leaf.path, "src/features/review/File.cs");
        assert!(leaf.is_viewed);
        assert_eq!(leaf.parent_path.as_deref(), Some(root.path.as_str()));
    }

    #[test]
    fn diff_rows_pair_changes_and_expose_changed_line_indices() {
        let lines = [
            DiffLine {
                kind: DiffLineKind::Context,
                old_line_number: Some(10),
                new_line_number: Some(10),
                text: "before".to_string(),
                ..Default::default()
            },
            DiffLine {
                kind: DiffLineKind::Deleted,
                old_line_number: Some(11),
                text: "old one".to_string(),
                ..Default::default()
            },
            DiffLine {
                kind: DiffLineKind::Deleted,
                old_line_number: Some(12),
                text: "old two".to_string(),
                ..Default::default()
            },
            DiffLine {
                kind: DiffLineKind::Added,
                new_line_number: Some(11),
                text: "new one".to_string(),
                ..Default::default()
            },
            DiffLine {
                kind: DiffLineKind::Hunk,
                text: "@@ -13 +12 @@".to_string(),
                ..Default::default()
            },
        ];
        let mut diff = DiffState::from_lines(lines);

        assert_eq!(diff.change_indices, [1, 2, 3]);
        assert_eq!(diff.selected_line_index, Some(1));
        assert_eq!(diff.change_position(), Some(1));
        assert_eq!(diff.navigate_change(1), Some(2));
        assert_eq!(diff.navigate_change(-1), Some(1));

        diff.set_layout(DiffLayout::SideBySide);
        assert_eq!(diff.rows.len(), 4);
        assert_eq!(diff.rows[0].left.as_ref().unwrap().line.text, "before");
        assert_eq!(
            diff.rows[0].left.as_ref().unwrap().source_index,
            diff.rows[0].right.as_ref().unwrap().source_index
        );
        assert_eq!(diff.rows[1].left.as_ref().unwrap().line.text, "old one");
        assert_eq!(diff.rows[1].right.as_ref().unwrap().line.text, "new one");
        assert_eq!(diff.rows[2].left.as_ref().unwrap().line.text, "old two");
        assert!(diff.rows[2].right.is_none());
        assert!(diff.rows[3].is_hunk());
        assert!(diff.find_row(2).is_some());
    }

    #[test]
    fn comment_readiness_and_edit_cancellation_are_independent_of_ui() {
        let mut session = review_state("comments", 4);
        session.comments.push(ReviewComment {
            id: "draft-1".to_string(),
            body: "Original body".to_string(),
            suggested_replacement: "Original suggestion".to_string(),
            status: ReviewCommentStatus::Pending,
            ..Default::default()
        });
        let mut review = ReviewSessionAppState::from_session_state(&session);
        review.replace_diff_lines([DiffLine {
            kind: DiffLineKind::Context,
            file_path: "src/File.cs".to_string(),
            new_line_number: Some(7),
            old_line_number: Some(7),
            text: "Call();".to_string(),
        }]);
        assert!(review.select_diff_line(0));

        assert!(!review.can_save_comment());
        review.draft_comment = "Please rename this.".to_string();
        assert!(review.can_save_comment());
        review.is_busy = true;
        assert!(!review.can_save_comment());
        review.is_busy = false;
        review.draft_comment.clear();
        review.prepare_focused_comment();
        assert_eq!(review.draft_suggestion, "Call();");
        assert!(!review.can_save_comment());
        review.draft_suggestion = "CallRenamed();".to_string();
        assert!(review.can_save_comment());

        assert!(review.begin_comment_edit("draft-1"));
        review.comments[0].edit_body = "Changed body".to_string();
        review.comments[0].edit_suggestion = "Changed suggestion".to_string();
        assert!(review.can_commit_comment_edit("draft-1"));
        assert!(review.cancel_comment_edit("draft-1"));
        let edited = &review.comments[0];
        assert!(!edited.is_editing);
        assert_eq!(edited.comment.body, "Original body");
        assert_eq!(edited.comment.suggested_replacement, "Original suggestion");
        assert!(edited.edit_body.is_empty());

        let queued_id = review
            .queue_comment()
            .expect("the changed suggestion is ready");
        assert!(review.draft_comment.is_empty());
        assert!(review.draft_suggestion.is_empty());
        assert!(review.selected_code_fragment.is_empty());
        assert!(review.mark_comment_published(&queued_id, "remote-1"));
        assert!(
            review
                .comments
                .iter()
                .find(|comment| comment.id() == queued_id)
                .is_some_and(LocalReviewCommentState::is_published)
        );
    }

    #[test]
    fn activity_month_navigation_and_aggregation_preserve_page_state() {
        let month = ActivityMonth::new(2026, 11).unwrap();
        let previous = month.previous();
        let day = NaiveDate::from_ymd_opt(2026, 11, 12).unwrap();
        let instant = Utc.with_ymd_and_hms(2026, 11, 12, 12, 0, 0).unwrap();
        let records = [
            ReviewActivityRecord {
                id: "opened".to_string(),
                session_key: "review-1".to_string(),
                kind: ReviewActivityKind::Opened,
                at_utc: instant,
                repository: "octo-org/widget".to_string(),
                ..Default::default()
            },
            ReviewActivityRecord {
                id: "comment".to_string(),
                session_key: "review-1".to_string(),
                kind: ReviewActivityKind::CommentPublished,
                at_utc: instant,
                repository: "octo-org/widget".to_string(),
                comment_delta: 1,
                ..Default::default()
            },
            ReviewActivityRecord {
                id: "reply".to_string(),
                session_key: "review-1".to_string(),
                kind: ReviewActivityKind::ReplyPublished,
                at_utc: instant,
                repository: "octo-org/widget".to_string(),
                comment_delta: 1,
                ..Default::default()
            },
            ReviewActivityRecord {
                id: "reviewed".to_string(),
                session_key: "review-1".to_string(),
                kind: ReviewActivityKind::MarkedReviewed,
                at_utc: instant,
                repository: "octo-org/widget".to_string(),
                ..Default::default()
            },
        ];
        let mut activity = ReviewActivityPageState::for_month(month);

        activity.aggregate_records(month, &records);
        assert_eq!(activity.days.len(), 30);
        let daily = activity.days.iter().find(|item| item.date == day).unwrap();
        assert_eq!(daily.pull_request_count, 1);
        assert_eq!(daily.comment_count, 2);
        assert_eq!(daily.review_count, 1);
        assert_eq!(activity.chart_maximum, 5);
        assert_eq!(activity.recent_reviews.len(), 1);

        assert_eq!(activity.previous_month(), previous);
        assert_eq!(activity.visible_month, previous);
        assert!(activity.can_move_to_next_month());
        assert_eq!(activity.next_month(), Some(month));
        assert!(!activity.can_move_to_next_month());
        assert_eq!(activity.days.len(), 30);
    }

    #[test]
    fn snapshot_refresh_preserves_the_active_diff_and_selection() {
        let mut session = review_state("preserve-view", 12);
        session.changed_files.push(ChangedFile {
            path: "src/Main.cs".to_string(),
            status: ChangedFileStatus::Modified,
            ..Default::default()
        });
        session.comments.push(ReviewComment {
            id: "local".to_string(),
            file_path: "src/Main.cs".to_string(),
            line_number: 7,
            body: "Original".to_string(),
            ..Default::default()
        });
        let mut review = ReviewSessionAppState::from_session_state(&session);
        assert!(review.select_file_or_folder("src/Main.cs"));
        review.replace_diff_lines([DiffLine {
            kind: DiffLineKind::Context,
            file_path: "src/Main.cs".to_string(),
            new_line_number: Some(7),
            old_line_number: Some(7),
            text: "Line seven".to_string(),
        }]);
        review.set_notes("Unsaved local note");
        review.draft_comment = "Unsaved comment draft".to_string();
        let expected_diff = review.diff.clone();
        session.details.title = "Updated title".to_string();
        session.notes = "Stale provider snapshot".to_string();
        session.comments[0].body = "Updated body".to_string();

        review.replace_session_state_preserving_view(&session);

        assert_eq!(review.title(), "Updated title");
        assert_eq!(review.selected_file_path, "src/Main.cs");
        assert_eq!(review.diff, expected_diff);
        assert_eq!(review.comments[0].comment.body, "Updated body");
        assert_eq!(review.notes, "Unsaved local note");
        assert_eq!(review.draft_comment, "Unsaved comment draft");
    }

    #[test]
    fn full_refresh_reselects_when_the_previous_file_disappears() {
        let mut session = review_state("refresh-file", 13);
        session.changed_files = vec![
            ChangedFile {
                path: "src/Old.cs".to_string(),
                status: ChangedFileStatus::Deleted,
                ..Default::default()
            },
            ChangedFile {
                path: "src/New.cs".to_string(),
                status: ChangedFileStatus::Added,
                ..Default::default()
            },
        ];
        let mut review = ReviewSessionAppState::from_session_state(&session);
        assert!(review.select_file_or_folder("src/Old.cs"));
        session.changed_files.remove(0);

        review.replace_session_state_preserving_local_edits(&session);

        assert_eq!(review.selected_file_path, "src/New.cs");
        assert!(review.diff.lines.is_empty());
    }
}
