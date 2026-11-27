use naui::{DialogResponse, Result, RichText, Tasks};
use prs_winui_projection::Microsoft::UI::Xaml::Controls::Primitives::ToggleButton;
use prs_winui_projection::Microsoft::UI::Xaml::Controls::{
    AppBarButton, Button as XamlButton, ComboBox as XamlComboBox, ContentDialog,
    ContentDialogClosingEventArgs, ContentDialogResult, FontIcon, Grid as XamlGrid,
    HyperlinkButton, InfoBar as XamlInfoBar, MenuFlyoutItem, ProgressBar as XamlProgressBar,
    SelectionChangedEventHandler, StackPanel, TextBlock, TextBox, TextChangedEventHandler,
    ToolTipService,
};
use prs_winui_projection::Microsoft::UI::Xaml::Documents::{Inline, Run};
use prs_winui_projection::Microsoft::UI::Xaml::Markup::XamlReader;
use prs_winui_projection::Microsoft::UI::Xaml::{
    Application, Automation::AutomationProperties, DependencyObject, FrameworkElement,
    HorizontalAlignment, RoutedEventHandler, SizeChangedEventHandler, Thickness, UIElement,
    VerticalAlignment, Visibility, Window as XamlWindow,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use windows::Foundation::{IReference, PropertyValue, TypedEventHandler};
use windows_core::{HSTRING, IInspectable, Interface};

const TITLE_BAR_BUTTON_XAML: &str = r#"<Button
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
    x:Name="OpenPullRequestTitleButton"
    Height="28"
    Padding="10,0"
    HorizontalAlignment="Right"
    VerticalAlignment="Center"
    Style="{StaticResource SubtleButtonStyle}">
    <StackPanel Orientation="Horizontal" Spacing="6">
        <FontIcon FontSize="12" Glyph="&#xE710;" VerticalAlignment="Center" />
        <TextBlock x:Name="OpenPullRequestTitleLabel" VerticalAlignment="Center" />
    </StackPanel>
</Button>"#;

const TITLE_BAR_DRAG_XAML: &str = r#"<Grid
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    Background="Transparent"
    HorizontalAlignment="Stretch"
    VerticalAlignment="Stretch" />"#;

pub(super) fn load<T>(xaml: &str) -> Result<T>
where
    T: Interface,
{
    XamlReader::Load(&HSTRING::from(xaml))
        .and_then(|value| value.cast::<T>())
        .map_err(|error| naui::Error::new("load the native WinUI XAML", error.to_string()))
}

pub(super) fn element<T>(root: &FrameworkElement, name: &str) -> Result<T>
where
    T: Interface,
{
    root.FindName(&HSTRING::from(name))
        .and_then(|value| value.cast::<T>())
        .map_err(|error| {
            naui::Error::new(
                "find the native WinUI control",
                format!("'{name}': {error}"),
            )
        })
}

pub(super) fn ui_element<T: Interface>(control: &T) -> Result<UIElement> {
    control
        .cast::<UIElement>()
        .map_err(|error| naui::Error::new("convert a native WinUI control", error.to_string()))
}

pub(super) fn set_visible_control<T: Interface>(control: &T, visible: bool) {
    if let Ok(element) = control.cast::<UIElement>() {
        let _ = element.SetVisibility(if visible {
            Visibility::Visible
        } else {
            Visibility::Collapsed
        });
    }
}

pub(super) fn set_accessible_label_on<T: Interface>(control: &T, label: &str) {
    if let Ok(dependency) = control.cast::<DependencyObject>() {
        let _ = AutomationProperties::SetName(&dependency, &HSTRING::from(label));
    }
}

pub(super) fn set_tooltip_on<T: Interface>(control: &T, tooltip: &str) {
    if let Ok(dependency) = control.cast::<DependencyObject>() {
        if tooltip.trim().is_empty() {
            let _ = ToolTipService::SetToolTip(&dependency, None::<&IInspectable>);
        } else if let Ok(value) = PropertyValue::CreateString(&HSTRING::from(tooltip)) {
            let _ = ToolTipService::SetToolTip(&dependency, &value);
        }
    }
}

fn resource(key: &str) -> Result<IInspectable> {
    let key = PropertyValue::CreateString(&HSTRING::from(key))
        .map_err(|error| naui::Error::new("create a resource key", error.to_string()))?;
    Application::Current()
        .and_then(|application| application.Resources())
        .and_then(|resources| resources.Lookup(&key))
        .map_err(|error| {
            naui::Error::new("look up a WinUI resource", format!("'{key:?}': {error}"))
        })
}

pub(crate) trait RawWidget {
    fn raw_element(&self) -> Result<UIElement>;

    fn request_focus(&self) -> bool
    where
        Self: Sized,
    {
        self.raw_element()
            .and_then(|element| {
                element.StartBringIntoView().map_err(|error| {
                    naui::Error::new("focus a native control", error.to_string())
                })?;
                element
                    .Focus(prs_winui_projection::Microsoft::UI::Xaml::FocusState::Programmatic)
                    .map_err(|error| naui::Error::new("focus a native control", error.to_string()))
            })
            .unwrap_or(false)
    }
}

impl<T> RawWidget for T
where
    T: naui::Widget,
{
    fn raw_element(&self) -> Result<UIElement> {
        Ok(self.as_backend().native_element())
    }
}

#[derive(Clone)]
pub(crate) struct Button {
    native: ButtonKind,
    tasks: Tasks,
    updating: Arc<AtomicBool>,
}

#[derive(Clone)]
enum ButtonKind {
    Button(XamlButton),
    AppBar(AppBarButton),
    Toggle(ToggleButton),
}

impl Button {
    pub(super) fn from_button(tasks: Tasks, native: XamlButton) -> Self {
        Self {
            native: ButtonKind::Button(native),
            tasks,
            updating: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(super) fn from_app_bar(tasks: Tasks, native: AppBarButton) -> Self {
        Self {
            native: ButtonKind::AppBar(native),
            tasks,
            updating: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(super) fn from_toggle(tasks: Tasks, native: ToggleButton) -> Self {
        Self {
            native: ButtonKind::Toggle(native),
            tasks,
            updating: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn set_text(&self, text: &str) {
        let value = PropertyValue::CreateString(&HSTRING::from(text));
        match (&self.native, value) {
            (ButtonKind::Button(native), Ok(value)) => {
                let _ = native.SetContent(&value);
            }
            (ButtonKind::AppBar(native), _) => {
                let _ = native.SetLabel(&HSTRING::from(text));
            }
            (ButtonKind::Toggle(native), Ok(value)) => {
                let _ = native.SetContent(&value);
            }
            _ => {}
        }
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        match &self.native {
            ButtonKind::Button(native) => {
                let _ = native.SetIsEnabled(enabled);
            }
            ButtonKind::AppBar(native) => {
                let _ = native.SetIsEnabled(enabled);
            }
            ButtonKind::Toggle(native) => {
                let _ = native.SetIsEnabled(enabled);
            }
        }
    }

    pub(crate) fn set_visible(&self, visible: bool) {
        set_visible_control(&self.as_inspectable(), visible);
    }

    pub(crate) fn set_accessible_label(&self, label: &str) {
        set_accessible_label_on(&self.as_inspectable(), label);
    }

    pub(crate) fn set_tooltip(&self, tooltip: &str) {
        set_tooltip_on(&self.as_inspectable(), tooltip);
    }

    pub(crate) fn set_checked(&self, checked: bool) {
        let ButtonKind::Toggle(native) = &self.native else {
            return;
        };
        let already = native
            .IsChecked()
            .ok()
            .and_then(|value| value.GetBoolean().ok());
        if already == Some(checked) {
            return;
        }
        self.updating.store(true, Ordering::Relaxed);
        if let Ok(value) = PropertyValue::CreateBoolean(checked)
            && let Ok(value) = value.cast::<IReference<bool>>()
        {
            let _ = native.SetIsChecked(&value);
        }
        self.updating.store(false, Ordering::Relaxed);
    }

    pub(crate) fn on_click(&self, mut f: impl FnMut() + 'static) {
        let sender = self.tasks.channel(move |()| f());
        let handler = RoutedEventHandler::new(move |_, _| {
            let _ = sender.send(());
            Ok(())
        });
        let _ = match &self.native {
            ButtonKind::Button(native) => native.Click(&handler),
            ButtonKind::AppBar(native) => native.Click(&handler),
            ButtonKind::Toggle(native) => native.Click(&handler),
        };
    }

    pub(crate) fn on_toggle(&self, mut f: impl FnMut(bool) + 'static) {
        let sender = self.tasks.channel(move |checked: bool| f(checked));
        let unchecked_sender = sender.clone();
        let ButtonKind::Toggle(native) = &self.native else {
            return;
        };
        let updating_checked = self.updating.clone();
        let checked = RoutedEventHandler::new(move |_, _| {
            if !updating_checked.load(Ordering::Relaxed) {
                let _ = sender.send(true);
            }
            Ok(())
        });
        let updating_unchecked = self.updating.clone();
        let unchecked = RoutedEventHandler::new(move |_, _| {
            if !updating_unchecked.load(Ordering::Relaxed) {
                let _ = unchecked_sender.send(false);
            }
            Ok(())
        });
        let _ = native.Checked(&checked);
        let _ = native.Unchecked(&unchecked);
    }

    fn as_inspectable(&self) -> IInspectable {
        match &self.native {
            ButtonKind::Button(native) => native.cast().expect("Button is IInspectable"),
            ButtonKind::AppBar(native) => native.cast().expect("AppBarButton is IInspectable"),
            ButtonKind::Toggle(native) => native.cast().expect("ToggleButton is IInspectable"),
        }
    }
}

impl RawWidget for Button {
    fn raw_element(&self) -> Result<UIElement> {
        self.as_inspectable()
            .cast()
            .map_err(|error| naui::Error::new("convert a native Button", error.to_string()))
    }
}

#[derive(Clone)]
pub(crate) struct MenuItem {
    native: MenuFlyoutItem,
    tasks: Tasks,
}

impl MenuItem {
    pub(super) fn from_native(tasks: Tasks, native: MenuFlyoutItem) -> Self {
        Self { native, tasks }
    }

    pub(crate) fn set_text(&self, text: &str) {
        let _ = self.native.SetText(&HSTRING::from(text));
    }

    pub(crate) fn set_visible(&self, visible: bool) {
        set_visible_control(&self.native, visible);
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        let _ = self.native.SetIsEnabled(enabled);
    }

    pub(crate) fn on_click(&self, mut f: impl FnMut() + 'static) {
        let sender = self.tasks.channel(move |()| f());
        let handler = RoutedEventHandler::new(move |_, _| {
            let _ = sender.send(());
            Ok(())
        });
        let _ = self.native.Click(&handler);
    }
}

#[derive(Clone)]
pub(crate) struct Hyperlink {
    native: HyperlinkButton,
    tasks: Tasks,
}

impl Hyperlink {
    pub(super) fn from_native(tasks: Tasks, native: HyperlinkButton) -> Self {
        Self { native, tasks }
    }

    pub(crate) fn set_text(&self, text: &str) {
        if let Ok(value) = PropertyValue::CreateString(&HSTRING::from(text)) {
            let _ = self.native.SetContent(&value);
        }
    }

    pub(crate) fn set_tooltip(&self, tooltip: &str) {
        set_tooltip_on(&self.native, tooltip);
    }

    pub(crate) fn set_visible(&self, visible: bool) {
        set_visible_control(&self.native, visible);
    }

    pub(crate) fn on_click(&self, mut f: impl FnMut() + 'static) {
        let sender = self.tasks.channel(move |()| f());
        let handler = RoutedEventHandler::new(move |_, _| {
            let _ = sender.send(());
            Ok(())
        });
        let _ = self.native.Click(&handler);
    }
}

#[derive(Clone)]
pub(crate) struct InfoBar {
    native: XamlInfoBar,
    tasks: Tasks,
}

impl InfoBar {
    pub(super) fn from_native(tasks: Tasks, native: XamlInfoBar) -> Self {
        Self { native, tasks }
    }

    pub(crate) fn set_title(&self, title: &str) {
        let _ = self.native.SetTitle(&HSTRING::from(title));
    }

    pub(crate) fn set_message(&self, message: &str) {
        let _ = self.native.SetMessage(&HSTRING::from(message));
    }

    pub(crate) fn set_open(&self, open: bool) {
        let _ = self.native.SetIsOpen(open);
    }

    pub(crate) fn on_close(&self, mut f: impl FnMut() + 'static) {
        let sender = self.tasks.channel(move |()| f());
        let handler = TypedEventHandler::<XamlInfoBar, IInspectable>::new(move |_, _| {
            let _ = sender.send(());
            Ok(())
        });
        let _ = self.native.CloseButtonClick(&handler);
    }
}

#[derive(Clone)]
pub(crate) struct Label {
    native: TextBlock,
}

impl Label {
    pub(super) fn from_native(native: TextBlock) -> Self {
        Self { native }
    }

    pub(crate) fn set_text(&self, text: &str) {
        let _ = self.native.SetText(&HSTRING::from(text));
    }

    pub(crate) fn set_visible(&self, visible: bool) {
        set_visible_control(&self.native, visible);
    }

    pub(crate) fn set_tooltip(&self, tooltip: &str) {
        set_tooltip_on(&self.native, tooltip);
    }

    pub(crate) fn set_rich_text(&self, rich: &RichText) {
        let Ok(inlines) = self.native.Inlines() else {
            self.set_text(&rich.plain_text());
            return;
        };
        let _ = inlines.Clear();
        for value in rich.runs() {
            let Ok(run) = Run::new() else {
                continue;
            };
            let _ = run.SetText(&HSTRING::from(value.text()));
            if value.is_bold() {
                let _ = run.SetFontWeight(windows::UI::Text::FontWeights::SemiBold().unwrap());
            }
            if let Some(key) = value.text_color().xaml_brush_key()
                && let Ok(value) = resource(key)
                && let Ok(brush) =
                    value.cast::<prs_winui_projection::Microsoft::UI::Xaml::Media::Brush>()
            {
                let _ = run.SetForeground(&brush);
            }
            if let Ok(inline) = run.cast::<Inline>() {
                let _ = inlines.Append(&inline);
            }
        }
    }
}

impl RawWidget for Label {
    fn raw_element(&self) -> Result<UIElement> {
        ui_element(&self.native)
    }
}

#[derive(Clone)]
pub(crate) struct TextInput {
    native: TextBox,
    tasks: Tasks,
}

impl TextInput {
    pub(super) fn from_native(tasks: Tasks, native: TextBox) -> Self {
        Self { native, tasks }
    }

    pub(crate) fn text(&self) -> String {
        self.native
            .Text()
            .map(|text| text.to_string())
            .unwrap_or_default()
    }

    pub(crate) fn set_text(&self, text: &str) {
        if self.text() != text {
            let _ = self.native.SetText(&HSTRING::from(text));
        }
    }

    pub(crate) fn set_placeholder(&self, text: &str) {
        let _ = self.native.SetPlaceholderText(&HSTRING::from(text));
    }

    pub(crate) fn set_header(&self, text: &str) {
        if let Ok(value) = PropertyValue::CreateString(&HSTRING::from(text)) {
            let _ = self.native.SetHeader(&value);
        }
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        let _ = self.native.SetIsEnabled(enabled);
    }

    pub(crate) fn set_accessible_label(&self, label: &str) {
        set_accessible_label_on(&self.native, label);
    }

    pub(crate) fn set_tooltip(&self, tooltip: &str) {
        set_tooltip_on(&self.native, tooltip);
    }

    pub(crate) fn on_change(&self, mut f: impl FnMut(&str) + 'static) {
        let sender = self.tasks.channel(move |text: String| f(&text));
        let native = self.native.clone();
        let handler = TextChangedEventHandler::new(move |_, _| {
            if let Ok(text) = native.Text() {
                let _ = sender.send(text.to_string());
            }
            Ok(())
        });
        let _ = self.native.TextChanged(&handler);
    }

    pub(crate) fn on_submit(&self, mut f: impl FnMut(&str) + 'static) {
        let sender = self.tasks.channel(move |text: String| f(&text));
        let native = self.native.clone();
        let handler = prs_winui_projection::Microsoft::UI::Xaml::Input::KeyEventHandler::new(
            move |_, args| {
                if args
                    .as_ref()
                    .and_then(|args| args.Key().ok())
                    .is_some_and(|key| key == windows::System::VirtualKey::Enter)
                    && let Ok(text) = native.Text()
                {
                    let _ = sender.send(text.to_string());
                }
                Ok(())
            },
        );
        let _ = self.native.KeyDown(&handler);
    }
}

impl RawWidget for TextInput {
    fn raw_element(&self) -> Result<UIElement> {
        ui_element(&self.native)
    }
}

#[derive(Clone)]
pub(crate) struct ComboBox {
    native: XamlComboBox,
    tasks: Tasks,
    items: Rc<RefCell<Vec<String>>>,
    updating: Arc<AtomicBool>,
}

impl ComboBox {
    pub(super) fn from_native(tasks: Tasks, native: XamlComboBox) -> Self {
        Self {
            native,
            tasks,
            items: Rc::new(RefCell::new(Vec::new())),
            updating: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn set_items<S: AsRef<str>>(&self, items: &[S]) {
        let next = items
            .iter()
            .map(|item| item.as_ref().to_string())
            .collect::<Vec<_>>();
        if *self.items.borrow() == next {
            return;
        }
        let Ok(collection) = self.native.Items() else {
            return;
        };
        self.updating.store(true, Ordering::Relaxed);
        let _ = collection.Clear();
        for item in &next {
            if let Ok(value) = PropertyValue::CreateString(&HSTRING::from(item.as_str())) {
                let _ = collection.Append(&value);
            }
        }
        *self.items.borrow_mut() = next;
        let _ = self.native.SetSelectedIndex(-1);
        self.updating.store(false, Ordering::Relaxed);
    }

    pub(crate) fn set_selected(&self, index: usize) {
        let target = index as i32;
        if self.native.SelectedIndex().ok() == Some(target) {
            return;
        }
        self.updating.store(true, Ordering::Relaxed);
        let _ = self.native.SetSelectedIndex(target);
        self.updating.store(false, Ordering::Relaxed);
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        let _ = self.native.SetIsEnabled(enabled);
    }

    pub(crate) fn set_accessible_label(&self, label: &str) {
        set_accessible_label_on(&self.native, label);
    }

    pub(crate) fn set_tooltip(&self, tooltip: &str) {
        set_tooltip_on(&self.native, tooltip);
    }

    pub(crate) fn on_select(&self, mut f: impl FnMut(usize) + 'static) {
        let sender = self.tasks.channel(move |index: usize| f(index));
        let native = self.native.clone();
        let updating = self.updating.clone();
        let handler = SelectionChangedEventHandler::new(move |_, _| {
            if !updating.load(Ordering::Relaxed)
                && let Ok(index) = native.SelectedIndex()
                && index >= 0
            {
                let _ = sender.send(index as usize);
            }
            Ok(())
        });
        let _ = self.native.SelectionChanged(&handler);
    }
}

impl RawWidget for ComboBox {
    fn raw_element(&self) -> Result<UIElement> {
        ui_element(&self.native)
    }
}

#[derive(Clone)]
pub(crate) struct ProgressBar {
    native: XamlProgressBar,
}

impl ProgressBar {
    pub(super) fn from_native(native: XamlProgressBar) -> Self {
        Self { native }
    }

    pub(crate) fn set_visible(&self, visible: bool) {
        set_visible_control(&self.native, visible);
    }
}

impl RawWidget for ProgressBar {
    fn raw_element(&self) -> Result<UIElement> {
        ui_element(&self.native)
    }
}

#[derive(Clone)]
pub(crate) struct Stack {
    native: StackPanel,
}

impl Stack {
    pub(super) fn from_native(native: StackPanel) -> Self {
        Self { native }
    }

    pub(crate) fn append(&self, child: &dyn RawWidget) {
        if let (Ok(children), Ok(element)) = (self.native.Children(), child.raw_element()) {
            let _ = children.Append(&element);
        }
    }

    pub(crate) fn append_element(&self, element: &UIElement) {
        if let Ok(children) = self.native.Children() {
            let _ = children.Append(element);
        }
    }

    pub(crate) fn clear(&self) {
        if let Ok(children) = self.native.Children() {
            let _ = children.Clear();
        }
    }
}

impl RawWidget for Stack {
    fn raw_element(&self) -> Result<UIElement> {
        ui_element(&self.native)
    }
}

#[derive(Clone)]
pub(crate) struct RawSheet {
    native: ContentDialog,
    window: XamlWindow,
    tasks: Tasks,
    is_open: Arc<AtomicBool>,
    response_handler: Rc<RefCell<Option<i64>>>,
}

impl RawSheet {
    pub(super) fn from_native(tasks: Tasks, window: XamlWindow, native: ContentDialog) -> Self {
        Self {
            native,
            window,
            tasks,
            is_open: Arc::new(AtomicBool::new(false)),
            response_handler: Default::default(),
        }
    }

    pub(crate) fn on_response(&self, mut f: impl FnMut(DialogResponse) + 'static) {
        let sender = self
            .tasks
            .channel(move |response: DialogResponse| f(response));
        let is_open = self.is_open.clone();
        let handler = TypedEventHandler::<ContentDialog, ContentDialogClosingEventArgs>::new(
            move |_, args| {
                is_open.store(false, Ordering::Relaxed);
                let result = args
                    .as_ref()
                    .and_then(|args| args.Result().ok())
                    .unwrap_or(ContentDialogResult::None);
                let response = match result {
                    ContentDialogResult::Primary => DialogResponse::Primary,
                    ContentDialogResult::Secondary => DialogResponse::Secondary,
                    _ => DialogResponse::Cancel,
                };
                let _ = sender.send(response);
                Ok(())
            },
        );
        if let Ok(token) = self.native.Closing(&handler) {
            *self.response_handler.borrow_mut() = Some(token);
        }
    }

    pub(crate) fn open(&self) {
        if self.is_open.load(Ordering::Relaxed) {
            return;
        }
        let root = self
            .window
            .Content()
            .and_then(|content| content.XamlRoot())
            .ok();
        let Some(root) = root else {
            return;
        };
        let _ = self.native.SetXamlRoot(&root);
        self.is_open.store(true, Ordering::Relaxed);
        let _ = self.native.ShowAsync();
    }

    pub(crate) fn hide(&self) {
        if self.is_open.load(Ordering::Relaxed) {
            self.is_open.store(false, Ordering::Relaxed);
            let _ = self.native.Hide();
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.is_open.load(Ordering::Relaxed)
    }

    pub(crate) fn set_button_enabled(&self, response: DialogResponse, enabled: bool) {
        match response {
            DialogResponse::Primary => {
                let _ = self.native.SetIsPrimaryButtonEnabled(enabled);
            }
            DialogResponse::Secondary => {
                let _ = self.native.SetIsSecondaryButtonEnabled(enabled);
            }
            DialogResponse::Cancel => {}
        }
    }
}

/// Integrates the app icon and right-aligned "Open PR" button directly into
/// Naui's native 32-DIP WinUI title bar row so the window has a single
/// Windows 11 Mica title bar aligned with the system caption controls.
pub(super) fn mount_title_bar_action(
    tasks: Tasks,
    native_window: &XamlWindow,
    label: &str,
    accessible_name: &str,
    tooltip: &str,
) -> Result<Button> {
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(dir) = exe_path.parent()
    {
        let icon = dir.join("Assets").join("AppIcon.ico");
        if icon.is_file()
            && let Ok(app_window) = native_window.AppWindow()
        {
            let _ = app_window.SetIcon(&HSTRING::from(icon.to_string_lossy().as_ref()));
        }
    }

    let button_native = load::<XamlButton>(TITLE_BAR_BUTTON_XAML)?;
    let button_element = button_native
        .cast::<FrameworkElement>()
        .map_err(|error| naui::Error::new("cast the title bar button", error.to_string()))?;
    if let Ok(text) = element::<TextBlock>(&button_element, "OpenPullRequestTitleLabel") {
        let _ = text.SetText(&HSTRING::from(label));
    }
    set_accessible_label_on(&button_native, accessible_name);
    set_tooltip_on(&button_native, tooltip);

    if let Ok(root) = native_window
        .Content()
        .and_then(|content| content.cast::<XamlGrid>())
        && let Ok(root_children) = root.Children()
        && let Ok(title_bar) = root_children.GetAt(0).and_then(|el| el.cast::<XamlGrid>())
        && let Ok(title_children) = title_bar.Children()
    {
        // Shift Naui's built-in title label slightly right and place an app icon before it.
        if let Ok(title_text) = title_children
            .GetAt(0)
            .and_then(|el| el.cast::<FrameworkElement>())
        {
            let _ = title_text.SetMargin(Thickness {
                Left: 22.0,
                Top: 0.0,
                Right: 0.0,
                Bottom: 0.0,
            });
        }
        if let Ok(icon) = FontIcon::new() {
            let _ = icon.SetGlyph(&HSTRING::from("\u{E8E5}"));
            let _ = icon.SetFontSize(14.0);
            let _ = icon.SetHorizontalAlignment(HorizontalAlignment::Left);
            let _ = icon.SetVerticalAlignment(VerticalAlignment::Center);
            if let Ok(icon_fe) = icon.cast::<FrameworkElement>() {
                let _ = XamlGrid::SetColumn(&icon_fe, 0);
            }
            if let Ok(icon_el) = icon.cast::<UIElement>() {
                let _ = title_children.Append(&icon_el);
            }
        }

        // Place a transparent drag strip across Columns 0..=1 (title + flexible empty space),
        // leaving Column 2 solely for the interactive "+ Open PR" button.
        if let Ok(drag_grid) = load::<XamlGrid>(TITLE_BAR_DRAG_XAML)
            && let Ok(drag_fe) = drag_grid.cast::<FrameworkElement>()
            && let Ok(drag_ui) = drag_grid.cast::<UIElement>()
        {
            let _ = XamlGrid::SetColumn(&drag_fe, 0);
            let _ = XamlGrid::SetColumnSpan(&drag_fe, 2);
            let _ = title_children.InsertAt(0, &drag_ui);

            // Column 2 holds Naui's fallback drag Grid; clear its MinWidth and place the button there.
            // Indices shifted by 1 after InsertAt(0), so Naui's Col 2 container is now at index 3.
            if let Ok(col2_host) = title_children.GetAt(3).and_then(|el| el.cast::<XamlGrid>()) {
                let _ = col2_host.SetMinWidth(0.0);
                let _ = col2_host.SetMargin(Thickness {
                    Left: 8.0,
                    Top: 0.0,
                    Right: 4.0,
                    Bottom: 0.0,
                });
                if let Ok(col2_children) = col2_host.Children()
                    && let Ok(btn_ui) = button_native.cast::<UIElement>()
                {
                    let _ = col2_children.Append(&btn_ui);
                }
            }

            // Re-apply SetTitleBar(&drag_ui) after Naui's synchronous SizeChanged drag-rect update
            // so the button stays clickable while the rest of the title bar remains draggable.
            let apply_window = native_window.clone();
            let apply_drag = drag_ui.clone();
            let apply_tasks = tasks.clone();
            let drag_sender = tasks.channel(move |()| {
                let window = apply_window.clone();
                let drag = apply_drag.clone();
                let _ = apply_tasks.after(Duration::ZERO, move || {
                    let _ = window.SetTitleBar(&drag);
                });
            });
            let _ = drag_sender.send(());
            let size_handler = SizeChangedEventHandler::new(move |_, _| {
                let _ = drag_sender.send(());
                Ok(())
            });
            let _ = title_bar.SizeChanged(&size_handler);
        }
    }

    Ok(Button::from_button(tasks, button_native))
}
