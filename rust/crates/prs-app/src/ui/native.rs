use super::raw::{self, Button, RawWidget};
use naui::{List, ListRow, Result, Sizing, Tasks, Widget as _};
use prs_winui_projection::Microsoft::UI::Xaml::Controls::{
    Border, ContentControl, Grid as XamlGrid, ListView, ListViewItem, NavigationView,
    NavigationViewItem, NavigationViewPaneDisplayMode, NavigationViewSelectionChangedEventArgs,
    ScrollBarVisibility, ScrollMode, ScrollViewer, SelectionChangedEventHandler, StackPanel,
    TextBlock,
};
use prs_winui_projection::Microsoft::UI::Xaml::Input::{KeyEventHandler, PointerEventHandler};
use prs_winui_projection::Microsoft::UI::Xaml::Media::VisualTreeHelper;
use prs_winui_projection::Microsoft::UI::Xaml::{
    DependencyObject, FrameworkElement, HorizontalAlignment, RoutedEventHandler,
    SizeChangedEventHandler, Thickness, UIElement,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use windows::Foundation::{IPropertyValue, PropertyValue, TypedEventHandler};
use windows::System::VirtualKey;
use windows_core::{HSTRING, IInspectable, Interface};

const NAVIGATION_ITEM_XAML: &str = include_str!("../../resources/NavigationItem.xaml");
const REVIEWED_NAVIGATION_ITEM_XAML: &str =
    include_str!("../../resources/ReviewedNavigationItem.xaml");
type IndexCallback = Rc<RefCell<Box<dyn FnMut(usize)>>>;

#[derive(Clone)]
pub(crate) struct Pane {
    native: UIElement,
}

impl Pane {
    pub(super) fn from_element(native: UIElement) -> Self {
        Self { native }
    }

    pub(crate) fn set_visible(&self, visible: bool) {
        raw::set_visible_control(&self.native, visible);
    }
}

impl RawWidget for Pane {
    fn raw_element(&self) -> Result<UIElement> {
        Ok(self.native.clone())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SidebarReviewItem {
    pub(crate) primary: String,
    pub(crate) secondary: String,
    pub(crate) tooltip: String,
    pub(crate) badge: Option<String>,
    pub(crate) status: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Toolbar {
    items: Vec<Option<Button>>,
}

impl Toolbar {
    pub(super) fn from_controls(items: Vec<Option<Button>>) -> Self {
        Self { items }
    }

    pub(crate) fn set_item_enabled(&self, index: usize, enabled: bool) {
        if let Some(button) = self.items.get(index).and_then(Option::as_ref) {
            button.set_enabled(enabled);
        }
    }

    pub(crate) fn on_activate(&self, f: impl FnMut(usize) + 'static) {
        let callback: IndexCallback = Rc::new(RefCell::new(Box::new(f)));
        for (index, button) in self.items.iter().enumerate() {
            if let Some(button) = button {
                let callback = callback.clone();
                button.on_click({
                    let callback = callback.clone();
                    move || (callback.borrow_mut())(index)
                });
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct Sidebar {
    native: NavigationView,
    reviewed_list: ListView,
    reviewed_panel: StackPanel,
    activity_button: Button,
    activity_label: TextBlock,
    has_reviewed: Arc<AtomicBool>,
    synchronizing: Arc<AtomicBool>,
    active_items: Rc<RefCell<Vec<NavigationViewItem>>>,
    reviewed_items: Rc<RefCell<Vec<FrameworkElement>>>,
    tasks: Tasks,
}

impl Sidebar {
    pub(super) fn from_native(
        tasks: Tasks,
        native: NavigationView,
        reviewed_list: ListView,
        reviewed_panel: StackPanel,
        activity_button: Button,
        activity_label: TextBlock,
    ) -> Self {
        let this = Self {
            native,
            reviewed_list,
            reviewed_panel,
            activity_button,
            activity_label,
            has_reviewed: Arc::new(AtomicBool::new(false)),
            synchronizing: Arc::new(AtomicBool::new(false)),
            active_items: Rc::new(RefCell::new(Vec::new())),
            reviewed_items: Rc::new(RefCell::new(Vec::new())),
            tasks,
        };
        this.install_pane_observers();
        this.sync_pane_footer(this.native.IsPaneOpen().unwrap_or(true));
        this
    }

    fn install_pane_observers(&self) {
        let label = self.activity_label.clone();
        let panel = self.reviewed_panel.clone();
        let has_reviewed = self.has_reviewed.clone();
        let footer_sender = self.tasks.channel(move |is_open: bool| {
            raw::set_visible_control(&label, is_open);
            raw::set_visible_control(&panel, is_open && has_reviewed.load(Ordering::Relaxed));
        });

        let opening_sender = footer_sender.clone();
        let opening = TypedEventHandler::<NavigationView, IInspectable>::new(move |_, _| {
            let _ = opening_sender.send(true);
            Ok(())
        });
        let _ = self.native.PaneOpening(&opening);

        let opened_sender = footer_sender.clone();
        let opened = TypedEventHandler::<NavigationView, IInspectable>::new(move |_, _| {
            let _ = opened_sender.send(true);
            Ok(())
        });
        let _ = self.native.PaneOpened(&opened);

        let closed_sender = footer_sender.clone();
        let closed = TypedEventHandler::<NavigationView, IInspectable>::new(move |_, _| {
            let _ = closed_sender.send(false);
            Ok(())
        });
        let _ = self.native.PaneClosed(&closed);

        let nav = self.native.clone();
        let sized = SizeChangedEventHandler::new(move |_, _| {
            let is_open = nav.IsPaneOpen().unwrap_or(false);
            let _ = footer_sender.send(is_open);
            Ok(())
        });
        let _ = self.native.SizeChanged(&sized);
    }

    fn sync_pane_footer(&self, is_open: bool) {
        raw::set_visible_control(&self.activity_label, is_open);
        raw::set_visible_control(
            &self.reviewed_panel,
            is_open && self.has_reviewed.load(Ordering::Relaxed),
        );
    }

    pub(crate) fn set_width(&self, width: f64) {
        let _ = self.native.SetOpenPaneLength(width);
    }

    pub(crate) fn set_sections(
        &self,
        active: &[SidebarReviewItem],
        reviewed: &[SidebarReviewItem],
    ) {
        self.synchronizing.store(true, Ordering::Relaxed);
        if let Ok(menu_items) = self.native.MenuItems() {
            let _ = menu_items.Clear();
            self.active_items.borrow_mut().clear();
            for item in active {
                let index = self.active_items.borrow().len();
                if let Ok(nav_item) = navigation_item(index, item) {
                    let _ = menu_items.Append(&nav_item);
                    self.active_items.borrow_mut().push(nav_item);
                }
            }
        }

        if let Ok(reviewed_items) = self.reviewed_list.Items() {
            let _ = reviewed_items.Clear();
            self.reviewed_items.borrow_mut().clear();
            for (offset, item) in reviewed.iter().enumerate() {
                let index = self.active_items.borrow().len() + offset;
                if let Ok(row) = reviewed_navigation_item(index, item) {
                    let _ = reviewed_items.Append(&row);
                    self.reviewed_items.borrow_mut().push(row);
                }
            }
        }
        self.has_reviewed
            .store(!reviewed.is_empty(), Ordering::Relaxed);
        self.sync_pane_footer(self.native.IsPaneOpen().unwrap_or(true));
        self.synchronizing.store(false, Ordering::Relaxed);
    }

    pub(crate) fn set_selected(&self, index: usize) {
        self.synchronizing.store(true, Ordering::Relaxed);
        let active_count = self.active_items.borrow().len();
        if index < active_count {
            let _ = self.reviewed_list.SetSelectedIndex(-1);
            if let Some(item) = self.active_items.borrow().get(index) {
                let _ = self.native.SetSelectedItem(item);
            }
            self.synchronizing.store(false, Ordering::Relaxed);
            return;
        }
        let reviewed_index = index - active_count;
        if reviewed_index < self.reviewed_items.borrow().len() {
            let _ = self.native.SetSelectedItem(None::<&IInspectable>);
            let _ = self.reviewed_list.SetSelectedIndex(reviewed_index as i32);
        } else {
            let _ = self.native.SetSelectedItem(None::<&IInspectable>);
            let _ = self.reviewed_list.SetSelectedIndex(-1);
        }
        self.synchronizing.store(false, Ordering::Relaxed);
    }

    pub(crate) fn clear_selection(&self) {
        self.synchronizing.store(true, Ordering::Relaxed);
        let _ = self.native.SetSelectedItem(None::<&IInspectable>);
        let _ = self.reviewed_list.SetSelectedIndex(-1);
        self.synchronizing.store(false, Ordering::Relaxed);
    }

    pub(crate) fn on_select(&self, mut f: impl FnMut(usize) + 'static) {
        let sender = self.tasks.channel(move |index: usize| f(index));
        let nav_sender = sender.clone();
        let navigation = self.native.clone();
        let nav_sync = self.synchronizing.clone();
        let handler =
            TypedEventHandler::<NavigationView, NavigationViewSelectionChangedEventArgs>::new(
                move |_, _| {
                    if !nav_sync.load(Ordering::Relaxed)
                        && let Ok(selected) = navigation.SelectedItem()
                        && let Ok(item) = selected.cast::<NavigationViewItem>()
                        && let Ok(tag) = item.Tag()
                        && let Ok(value) = tag.cast::<IPropertyValue>()
                        && let Ok(index) = value.GetUInt32()
                    {
                        let _ = nav_sender.send(index as usize);
                    }
                    Ok(())
                },
            );
        let _ = self.native.SelectionChanged(&handler);

        let reviewed_sender = sender.clone();
        let list = self.reviewed_list.clone();
        let reviewed_sync = self.synchronizing.clone();
        let reviewed_handler = SelectionChangedEventHandler::new(move |_, _| {
            if !reviewed_sync.load(Ordering::Relaxed)
                && let Ok(selected) = list.SelectedItem()
                && let Ok(item) = selected.cast::<FrameworkElement>()
                && let Ok(tag) = item.Tag()
                && let Ok(value) = tag.cast::<IPropertyValue>()
                && let Ok(index) = value.GetUInt32()
            {
                let _ = reviewed_sender.send(index as usize);
            }
            Ok(())
        });
        let _ = self.reviewed_list.SelectionChanged(&reviewed_handler);

        self.activity_button.on_click({
            let active = self.active_items.clone();
            let reviewed = self.reviewed_items.clone();
            move || {
                let _ = sender.send(active.borrow().len() + reviewed.borrow().len());
            }
        });
    }

    pub(crate) fn set_collapsed(&self, collapsed: bool) {
        let _ = self.native.SetPaneDisplayMode(if collapsed {
            NavigationViewPaneDisplayMode::LeftCompact
        } else {
            NavigationViewPaneDisplayMode::Auto
        });
        let is_open = if collapsed {
            false
        } else {
            self.native.IsPaneOpen().unwrap_or(true)
        };
        self.sync_pane_footer(is_open);
    }
}

fn navigation_item(index: usize, item: &SidebarReviewItem) -> Result<NavigationViewItem> {
    let row = raw::load::<FrameworkElement>(NAVIGATION_ITEM_XAML)?;
    if let Ok(text) = raw::element::<TextBlock>(&row, "PrimaryTitle") {
        let _ = text.SetText(&HSTRING::from(item.primary.as_str()));
    }
    if let Ok(text) = raw::element::<TextBlock>(&row, "SecondaryTitle") {
        let _ = text.SetText(&HSTRING::from(item.secondary.as_str()));
    }
    if let Ok(badge_border) = raw::element::<FrameworkElement>(&row, "NewIterationBadge") {
        raw::set_visible_control(&badge_border, item.badge.is_some());
    }
    if let (Some(badge), Ok(text)) = (
        item.badge.as_deref(),
        raw::element::<TextBlock>(&row, "NewIterationText"),
    ) {
        let _ = text.SetText(&HSTRING::from(badge));
    }
    let nav_item = NavigationViewItem::new()
        .map_err(|error| naui::Error::new("create a NavigationViewItem", error.to_string()))?;
    nav_item
        .SetContent(
            &row.cast::<IInspectable>()
                .expect("navigation row is IInspectable"),
        )
        .map_err(|error| naui::Error::new("set NavigationViewItem content", error.to_string()))?;
    if let Ok(tag) = PropertyValue::CreateUInt32(index as u32) {
        let _ = nav_item.SetTag(&tag);
    }
    raw::set_accessible_label_on(&nav_item, &item.primary);
    raw::set_tooltip_on(&nav_item, &item.tooltip);
    let _ = nav_item.SetHeight(48.0);
    let _ = nav_item.SetMinHeight(48.0);
    Ok(nav_item)
}

fn reviewed_navigation_item(index: usize, item: &SidebarReviewItem) -> Result<FrameworkElement> {
    let row = raw::load::<FrameworkElement>(REVIEWED_NAVIGATION_ITEM_XAML)?;
    if let Ok(text) = raw::element::<TextBlock>(&row, "PrimaryTitle") {
        let _ = text.SetText(&HSTRING::from(item.primary.as_str()));
    }
    if let Ok(text) = raw::element::<TextBlock>(&row, "SecondaryTitle") {
        let _ = text.SetText(&HSTRING::from(item.secondary.as_str()));
    }
    if let Ok(text) = raw::element::<TextBlock>(&row, "StatusText") {
        let _ = text.SetText(&HSTRING::from(item.status.as_deref().unwrap_or("")));
    }
    if let Ok(tag) = PropertyValue::CreateUInt32(index as u32) {
        let _ = row.SetTag(&tag);
    }
    raw::set_accessible_label_on(&row, &item.primary);
    raw::set_tooltip_on(&row, &item.tooltip);
    Ok(row)
}

#[derive(Clone)]
pub(crate) struct Tabs {
    buttons: [Button; 3],
    panes: [Pane; 3],
    selected: Rc<Cell<usize>>,
}

impl Tabs {
    pub(super) fn new(buttons: [Button; 3], panes: [Pane; 3]) -> Self {
        let this = Self {
            buttons,
            panes,
            selected: Rc::new(Cell::new(0)),
        };
        this.set_selected(0);
        this
    }

    pub(crate) fn set_selected(&self, index: usize) {
        let index = index.min(2);
        self.selected.set(index);
        for (position, button) in self.buttons.iter().enumerate() {
            button.set_checked(position == index);
        }
        for (position, pane) in self.panes.iter().enumerate() {
            pane.set_visible(position == index);
        }
    }

    pub(crate) fn on_select(&self, f: impl FnMut(usize) + 'static) {
        let callback: IndexCallback = Rc::new(RefCell::new(Box::new(f)));
        for (index, button) in self.buttons.iter().enumerate() {
            let callback = callback.clone();
            let tabs = self.clone();
            button.on_click(move || {
                tabs.set_selected(index);
                (callback.borrow_mut())(index);
            });
        }
    }
}

#[derive(Clone)]
pub(crate) struct DiffList {
    list: List,
    scroll: Option<ScrollViewer>,
    body: Option<FrameworkElement>,
    side_by_side_min_width: Arc<Mutex<Option<f64>>>,
}

impl DiffList {
    pub(super) fn new(ui: &naui::Ui, host: XamlGrid) -> Result<Self> {
        let list = ui.list()?;
        list.set_sizing(Sizing::fill());
        list.set_row_height(24.0);
        let native = list.as_backend().native_element();
        let side_by_side_min_width = Arc::new(Mutex::new(None));
        let (scroll, body) = configure_diff_list_surface(&native, side_by_side_min_width.clone());
        host.Children()
            .map_err(|error| naui::Error::new("get the diff host children", error.to_string()))?
            .Append(&native)
            .map_err(|error| {
                naui::Error::new("host the native diff ListView", error.to_string())
            })?;
        Ok(Self {
            list,
            scroll,
            body,
            side_by_side_min_width,
        })
    }

    pub(crate) fn set_side_by_side_min_width(&self, min_width: Option<f64>) {
        if let Ok(mut guard) = self.side_by_side_min_width.lock() {
            *guard = min_width;
        }
        if let (Some(scroll), Some(body)) = (&self.scroll, &self.body) {
            sync_diff_body_width(scroll, body, min_width);
        }
    }

    pub(crate) fn set_row_builder(
        &self,
        count: usize,
        mut build: impl FnMut(usize) -> Result<ListRow> + 'static,
    ) {
        self.list.set_row_builder(count, move |index| {
            build(index).inspect_err(|error| {
                eprintln!("prs-app: failed to build diff row {index}: {error}");
            })
        });
    }

    pub(crate) fn set_selected(&self, row: usize) {
        self.list.set_selected(row);
    }

    pub(crate) fn scroll_to_row(&self, row: usize) {
        self.list.scroll_to_row(row);
    }

    pub(crate) fn on_select(&self, f: impl FnMut(&[usize]) + 'static) {
        self.list.on_select(f);
    }
}

fn sync_diff_body_width(
    scroll: &ScrollViewer,
    body: &FrameworkElement,
    side_by_side_min_width: Option<f64>,
) {
    let viewport = scroll
        .ViewportWidth()
        .ok()
        .filter(|width| *width > 0.0)
        .or_else(|| scroll.ActualWidth().ok().filter(|width| *width > 0.0));
    match side_by_side_min_width {
        Some(min_width) => {
            // Side-by-side rows split equally with `* | 1 | *`. Inside a horizontal
            // ScrollViewer, Grid `*` columns collapse to Auto unless the scrolled body
            // carries an explicit finite Width, so assign the larger of the viewport
            // and the measured two-column content width.
            let target = viewport.map_or(min_width, |view| view.max(min_width));
            let _ = body.SetMinWidth(target);
            let _ = body.SetWidth(target);
        }
        None => {
            let _ = body.SetWidth(f64::NAN);
            if let Some(view) = viewport {
                let _ = body.SetMinWidth(view);
            }
        }
    }
}

fn configure_diff_list_surface(
    native: &UIElement,
    side_by_side_min_width: Arc<Mutex<Option<f64>>>,
) -> (Option<ScrollViewer>, Option<FrameworkElement>) {
    let Ok(border) = native.cast::<Border>() else {
        return (None, None);
    };
    let _ = border.SetPadding(Thickness {
        Left: 0.0,
        Top: 0.0,
        Right: 0.0,
        Bottom: 0.0,
    });
    let _ = border.SetBorderThickness(Thickness {
        Left: 0.0,
        Top: 0.0,
        Right: 0.0,
        Bottom: 0.0,
    });
    let _ = border.ClearValue(&Border::BackgroundProperty().expect("BackgroundProperty"));
    let Ok(scroll) = border
        .Child()
        .and_then(|child| child.cast::<ScrollViewer>())
    else {
        return (None, None);
    };
    let _ = scroll.SetHorizontalScrollMode(ScrollMode::Enabled);
    let _ = scroll.SetHorizontalScrollBarVisibility(ScrollBarVisibility::Auto);
    let Ok(body) = scroll
        .Content()
        .and_then(|content| content.cast::<FrameworkElement>())
    else {
        return (Some(scroll), None);
    };
    let scroll_clone = scroll.clone();
    let body_clone = body.clone();
    let width_state = side_by_side_min_width.clone();
    sync_diff_body_width(&scroll, &body, None);
    let handler = SizeChangedEventHandler::new(move |_, _| {
        let sbs = width_state.lock().ok().and_then(|guard| *guard);
        sync_diff_body_width(&scroll_clone, &body_clone, sbs);
        Ok(())
    });
    let _ = scroll.SizeChanged(&handler);
    (Some(scroll), Some(body))
}

/// Removes Naui's default list-row vertical padding (8 DIP top/bottom) from the
/// enclosing `ListViewItem` once the diff row enters the visual tree so diff
/// backgrounds and accent strokes stay continuous at 24-DIP code line density.
pub(crate) fn normalize_diff_row_container(root: &FrameworkElement) {
    let loaded = RoutedEventHandler::new(move |sender, _| {
        let Some(content) = sender
            .as_ref()
            .and_then(|value| value.cast::<DependencyObject>().ok())
        else {
            return Ok(());
        };
        let mut current = VisualTreeHelper::GetParent(&content).ok();
        while let Some(element) = current {
            if let Ok(item) = element.cast::<ListViewItem>() {
                let _ = item.SetPadding(Thickness {
                    Left: 0.0,
                    Top: 0.0,
                    Right: 0.0,
                    Bottom: 0.0,
                });
                let _ = item.SetMinHeight(24.0);
                let _ = item.SetHorizontalContentAlignment(HorizontalAlignment::Stretch);
                break;
            }
            current = VisualTreeHelper::GetParent(&element).ok();
        }
        Ok(())
    });
    let _ = root.Loaded(&loaded);
}

#[derive(Clone)]
pub(crate) struct PaneSplitter {
    native: ContentControl,
    tasks: Tasks,
}

impl PaneSplitter {
    pub(super) fn from_native(tasks: Tasks, native: ContentControl) -> Self {
        Self { native, tasks }
    }

    pub(crate) fn set_visible(&self, visible: bool) {
        raw::set_visible_control(&self.native, visible);
    }

    pub(crate) fn set_accessible_label(&self, label: &str) {
        raw::set_accessible_label_on(&self.native, label);
        raw::set_tooltip_on(&self.native, label);
    }

    pub(crate) fn on_resize(&self, mut f: impl FnMut(f64) + 'static) {
        let sender = self.tasks.channel(move |delta: f64| f(delta));
        let dragging: Arc<Mutex<Option<(u32, f64)>>> = Arc::new(Mutex::new(None));

        let pressed_drag = dragging.clone();
        let pressed_control = self.native.clone();
        let pressed = PointerEventHandler::new(move |_, args| {
            let Some(args) = args.as_ref() else {
                return Ok(());
            };
            let Ok(pointer) = args.Pointer() else {
                return Ok(());
            };
            let Ok(point) = args.GetCurrentPoint(None) else {
                return Ok(());
            };
            let Ok(position) = point.Position() else {
                return Ok(());
            };
            let id = pointer.PointerId().unwrap_or(0);
            if pressed_control.CapturePointer(&pointer).unwrap_or(false)
                && let Ok(mut guard) = pressed_drag.lock()
            {
                *guard = Some((id, f64::from(position.X)));
                let _ = args.SetHandled(true);
            }
            Ok(())
        });
        let _ = self.native.PointerPressed(&pressed);

        let moved_drag = dragging.clone();
        let moved_sender = sender.clone();
        let moved = PointerEventHandler::new(move |_, args| {
            let Some(args) = args.as_ref() else {
                return Ok(());
            };
            let id = args
                .Pointer()
                .and_then(|pointer| pointer.PointerId())
                .unwrap_or(0);
            let Ok(mut guard) = moved_drag.lock() else {
                return Ok(());
            };
            let Some((active_id, last_x)) = *guard else {
                return Ok(());
            };
            if id != active_id {
                return Ok(());
            }
            if let Ok(point) = args.GetCurrentPoint(None)
                && let Ok(position) = point.Position()
            {
                let next_x = f64::from(position.X);
                let delta = next_x - last_x;
                if delta.abs() >= 1.0 {
                    *guard = Some((active_id, next_x));
                    let _ = moved_sender.send(delta);
                }
                let _ = args.SetHandled(true);
            }
            Ok(())
        });
        let _ = self.native.PointerMoved(&moved);

        let release_drag = dragging.clone();
        let release_control = self.native.clone();
        let released = PointerEventHandler::new(move |_, args| {
            let Some(args) = args.as_ref() else {
                return Ok(());
            };
            let Ok(pointer) = args.Pointer() else {
                return Ok(());
            };
            let id = pointer.PointerId().unwrap_or(0);
            let should_release = release_drag.lock().ok().is_some_and(|mut guard| {
                if guard.is_some_and(|(active_id, _)| active_id == id) {
                    *guard = None;
                    true
                } else {
                    false
                }
            });
            if should_release {
                let _ = release_control.ReleasePointerCapture(&pointer);
                let _ = args.SetHandled(true);
            }
            Ok(())
        });
        let _ = self.native.PointerReleased(&released);

        let cancel_drag = dragging.clone();
        let cancel_control = self.native.clone();
        let canceled = PointerEventHandler::new(move |_, args| {
            let had_drag = cancel_drag
                .lock()
                .ok()
                .is_some_and(|mut guard| guard.take().is_some());
            if had_drag
                && let Some(args) = args.as_ref()
                && let Ok(pointer) = args.Pointer()
            {
                let _ = cancel_control.ReleasePointerCapture(&pointer);
            }
            Ok(())
        });
        let _ = self.native.PointerCanceled(&canceled);

        let lost_drag = dragging;
        let capture_lost = PointerEventHandler::new(move |_, _| {
            if let Ok(mut guard) = lost_drag.lock() {
                *guard = None;
            }
            Ok(())
        });
        let _ = self.native.PointerCaptureLost(&capture_lost);

        let key_sender = sender;
        let key_down = KeyEventHandler::new(move |_, args| {
            let Some(args) = args.as_ref() else {
                return Ok(());
            };
            let delta = match args.Key().unwrap_or(VirtualKey::None) {
                VirtualKey::Left => Some(-16.0),
                VirtualKey::Right => Some(16.0),
                _ => None,
            };
            if let Some(delta) = delta {
                let _ = key_sender.send(delta);
                let _ = args.SetHandled(true);
            }
            Ok(())
        });
        let _ = self.native.KeyDown(&key_down);
    }
}

pub(super) fn host_naui_widget(host: &XamlGrid, widget: &impl naui::Widget) -> Result<()> {
    host.Children()
        .map_err(|error| naui::Error::new("get host children", error.to_string()))?
        .Append(&widget.as_backend().native_element())
        .map_err(|error| naui::Error::new("host a native WinUI widget", error.to_string()))
}
