use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use serde::Serialize;
use tauri::webview::{NewWindowResponse, WebviewBuilder};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, PhysicalSize, Runtime, Webview, WebviewUrl,
    Window, WindowEvent,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

use crate::server::ServerState;

/// Height of the injected tab strip in logical pixels.
const TAB_BAR_HEIGHT: f64 = 38.0;
/// Custom scheme that serves the tab strip document.
pub(crate) const SCHEME: &str = "mixchat-tabs";
/// Query parameter that marks a page as loaded by the desktop shell.
const SHELL_QUERY: &str = "mixchat-shell";
const MAX_TITLE: usize = 120;

/// Owns every workspace window and the tabs inside it.
pub struct Tabs {
    state: Mutex<State>,
    main_taken: AtomicBool,
    next_window: AtomicU64,
    next_tab: AtomicU64,
}

#[derive(Default)]
struct State {
    workspaces: HashMap<String, Workspace>,
}

struct Workspace {
    tabs: Vec<Tab>,
    active: String,
}

struct Tab {
    id: String,
    webview: String,
    title: String,
    muted: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TabsView {
    window: String,
    active: String,
    tabs: Vec<TabView>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TabView {
    id: String,
    title: String,
    muted: bool,
}

enum CloseOutcome {
    Missing,
    CloseWindow,
    Closed { next: String, removed: String },
}

impl Tabs {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            main_taken: AtomicBool::new(false),
            next_window: AtomicU64::new(0),
            next_tab: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn allocate_window_label(&self) -> String {
        if !self.main_taken.swap(true, Ordering::SeqCst) {
            "main".to_string()
        } else {
            format!(
                "window-{}",
                self.next_window.fetch_add(1, Ordering::SeqCst) + 2
            )
        }
    }

    /// Hand a reserved label back when no window ever claimed it, so a later
    /// attempt can still use `main`. Only safe before the window exists: once
    /// Tauri holds the label, closing it asynchronously would race a re-allocation.
    fn release_window_label(&self, window: &str) {
        if window == "main" {
            self.main_taken.store(false, Ordering::SeqCst);
        }
    }

    fn allocate_tab_id(&self) -> String {
        format!("tab-{}", self.next_tab.fetch_add(1, Ordering::SeqCst) + 1)
    }

    fn insert_workspace(&self, window: &str, tab: Tab) {
        let active = tab.id.clone();
        self.lock().workspaces.insert(
            window.to_string(),
            Workspace {
                tabs: vec![tab],
                active,
            },
        );
    }

    fn remove_workspace(&self, window: &str) -> bool {
        self.lock().workspaces.remove(window).is_some()
    }

    /// Resolve the window and tab a content webview belongs to.
    fn workspace_for_content(&self, webview_label: &str) -> Option<(String, String)> {
        let state = self.lock();
        for (window, workspace) in &state.workspaces {
            for tab in &workspace.tabs {
                if tab.webview == webview_label {
                    return Some((window.clone(), tab.id.clone()));
                }
            }
        }
        None
    }

    /// Resolve a window from either a tab strip label or a content webview label.
    fn window_for_shell(&self, label: &str) -> Option<String> {
        if let Some(window) = label.strip_prefix("tabs-") {
            return self
                .lock()
                .workspaces
                .contains_key(window)
                .then(|| window.to_string());
        }
        self.workspace_for_content(label).map(|(window, _)| window)
    }

    /// Workspace labels in a stable order: `main` first, then sorted. HashMap
    /// iteration order must not decide which window an update or a menu action
    /// lands on.
    fn workspace_labels(&self) -> Vec<String> {
        let state = self.lock();
        let mut labels: Vec<String> = state.workspaces.keys().cloned().collect();
        labels.sort();
        if let Some(index) = labels.iter().position(|label| label == "main") {
            let main = labels.remove(index);
            labels.insert(0, main);
        }
        labels
    }

    fn webview_label(&self, window: &str, tab_id: &str) -> Option<String> {
        let state = self.lock();
        state
            .workspaces
            .get(window)?
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.webview.clone())
    }

    fn active_id(&self, window: &str) -> Option<String> {
        self.lock()
            .workspaces
            .get(window)
            .map(|workspace| workspace.active.clone())
    }

    fn active_webview(&self, window: &str) -> Option<String> {
        let state = self.lock();
        let workspace = state.workspaces.get(window)?;
        workspace
            .tabs
            .iter()
            .find(|tab| tab.id == workspace.active)
            .map(|tab| tab.webview.clone())
    }

    fn view(&self, window: &str) -> Option<TabsView> {
        let state = self.lock();
        let workspace = state.workspaces.get(window)?;
        Some(TabsView {
            window: window.to_string(),
            active: workspace.active.clone(),
            tabs: workspace
                .tabs
                .iter()
                .map(|tab| TabView {
                    id: tab.id.clone(),
                    title: tab.title.clone(),
                    muted: tab.muted,
                })
                .collect(),
        })
    }

    fn set_title(&self, window: &str, tab_id: &str, title: &str) -> bool {
        let mut state = self.lock();
        let Some(workspace) = state.workspaces.get_mut(window) else {
            return false;
        };
        let Some(tab) = workspace.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return false;
        };
        if tab.title == title {
            return false;
        }
        tab.title = title.to_string();
        true
    }

    fn toggle_mute(&self, window: &str, tab_id: &str) -> Option<(String, bool)> {
        let mut state = self.lock();
        let workspace = state.workspaces.get_mut(window)?;
        let tab = workspace.tabs.iter_mut().find(|tab| tab.id == tab_id)?;
        tab.muted = !tab.muted;
        Some((tab.webview.clone(), tab.muted))
    }

    fn close_tab(&self, window: &str, tab_id: &str) -> CloseOutcome {
        let mut state = self.lock();
        let Some(workspace) = state.workspaces.get_mut(window) else {
            return CloseOutcome::Missing;
        };
        // Resolve the tab before deciding to close the window: a stale id from
        // a tab strip that has not re-rendered yet must not tear down a window
        // whose only remaining tab is a different one.
        let Some(index) = workspace.tabs.iter().position(|tab| tab.id == tab_id) else {
            return CloseOutcome::Missing;
        };
        if workspace.tabs.len() <= 1 {
            return CloseOutcome::CloseWindow;
        }
        let removed = workspace.tabs.remove(index);
        let next = if workspace.active == tab_id {
            let index = index.min(workspace.tabs.len() - 1);
            workspace.tabs[index].id.clone()
        } else {
            workspace.active.clone()
        };
        workspace.active = next.clone();
        CloseOutcome::Closed {
            next,
            removed: removed.webview,
        }
    }

    fn reserve_tab(&self, window: &str, tab: Tab) -> Option<String> {
        let mut state = self.lock();
        let workspace = state.workspaces.get_mut(window)?;
        let previous = std::mem::replace(&mut workspace.active, tab.id.clone());
        workspace.tabs.push(tab);
        Some(previous)
    }

    /// Drop a tab that failed to build and put `restore` back in charge.
    fn rollback_tab(&self, window: &str, tab_id: &str, restore: &str) -> Option<String> {
        let mut state = self.lock();
        let workspace = state.workspaces.get_mut(window)?;
        workspace.tabs.retain(|tab| tab.id != tab_id);
        if workspace.active == tab_id {
            workspace.active = if workspace.tabs.iter().any(|tab| tab.id == restore) {
                restore.to_string()
            } else {
                workspace.tabs.last()?.id.clone()
            };
        }
        Some(workspace.active.clone())
    }

    fn cycle(&self, window: &str, step: isize) -> Option<String> {
        let state = self.lock();
        let workspace = state.workspaces.get(window)?;
        let len = workspace.tabs.len();
        if len == 0 {
            return None;
        }
        let current = workspace
            .tabs
            .iter()
            .position(|tab| tab.id == workspace.active)
            .unwrap_or(0);
        let next = (current as isize + step).rem_euclid(len as isize) as usize;
        Some(workspace.tabs[next].id.clone())
    }

    /// Jump to a 1-based tab number. Out-of-range numbers clamp to the last
    /// tab, matching how browsers treat `Ctrl+9`.
    fn select_index(&self, window: &str, index: usize) -> Option<String> {
        let state = self.lock();
        let workspace = state.workspaces.get(window)?;
        if workspace.tabs.is_empty() {
            return None;
        }
        let last = workspace.tabs.len();
        let index = index.checked_sub(1)?.min(last - 1);
        workspace.tabs.get(index).map(|tab| tab.id.clone())
    }

    fn content_labels(&self) -> Vec<String> {
        let state = self.lock();
        state
            .workspaces
            .values()
            .flat_map(|workspace| workspace.tabs.iter().map(|tab| tab.webview.clone()))
            .collect()
    }
}

fn tabs<R: Runtime>(app: &AppHandle<R>) -> Option<tauri::State<'_, Tabs>> {
    app.try_state::<Tabs>()
}

fn strip_label(window: &str) -> String {
    format!("tabs-{window}")
}

fn content_label(window: &str, tab_id: &str) -> String {
    format!("content-{window}-{tab_id}")
}

fn strip_url() -> tauri::Url {
    #[cfg(any(windows, target_os = "android"))]
    {
        "http://mixchat-tabs.localhost/index.html"
            .parse()
            .expect("valid tab strip URL")
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        "mixchat-tabs://localhost/index.html"
            .parse()
            .expect("valid tab strip URL")
    }
}

fn is_strip_url(url: &tauri::Url) -> bool {
    url.scheme() == SCHEME || url.host_str() == Some("mixchat-tabs.localhost")
}

fn server_url<R: Runtime>(app: &AppHandle<R>) -> String {
    app.try_state::<Arc<ServerState>>()
        .map(|state| state.url().to_string())
        .unwrap_or_else(|| "http://localhost:3000".to_string())
}

fn app_origin<R: Runtime>(app: &AppHandle<R>) -> String {
    server_url(app)
        .parse::<tauri::Url>()
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_default()
}

fn content_url<R: Runtime>(app: &AppHandle<R>, target: Option<&str>) -> tauri::Url {
    let base: tauri::Url = server_url(app)
        .parse()
        .unwrap_or_else(|_| "http://localhost:3000".parse().unwrap());
    let origin = app_origin(app);
    let mut url = match target {
        // A content webview carries the shell init script and the whole app
        // command surface, so it may only ever show the local server. Anything
        // else falls back to the base URL rather than loading a remote page.
        Some(target) if target.starts_with("http://") || target.starts_with("https://") => target
            .parse::<tauri::Url>()
            .ok()
            .filter(|url| url.origin().ascii_serialization() == origin)
            .unwrap_or_else(|| base.clone()),
        Some(target) => base
            .join(target.trim_start_matches('/'))
            .unwrap_or_else(|_| base.clone()),
        None => base.clone(),
    };
    if url.origin().ascii_serialization() == origin {
        let marked = url.query_pairs().any(|(key, _)| key == SHELL_QUERY);
        if !marked {
            url.query_pairs_mut().append_pair(SHELL_QUERY, "tauri");
        }
    }
    url
}

fn content_bounds<R: Runtime>(window: &Window<R>) -> (f64, f64) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let size = window
        .inner_size()
        .unwrap_or_else(|_| PhysicalSize::new(1600, 900));
    let width = (size.width as f64 / scale).max(1.0);
    let height = (size.height as f64 / scale).max(TAB_BAR_HEIGHT + 1.0);
    (width, height - TAB_BAR_HEIGHT)
}

/// Characters that can visually reorder or spoof a tab label.
fn is_bidi_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

fn clean_title(raw: &str) -> String {
    let title: String = raw
        .chars()
        .filter(|ch| !ch.is_control() && !is_bidi_control(*ch))
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_TITLE)
        .collect();
    if title.is_empty() {
        "Mixchat".to_string()
    } else {
        title
    }
}

fn window_title(title: &str) -> String {
    let title = title.trim();
    if title.is_empty() || title.eq_ignore_ascii_case("mixchat") {
        "Mixchat".to_string()
    } else {
        format!("Mixchat - {title}")
    }
}

fn content_init_script() -> String {
    concat!(
        r#"window.__MIXCHAT_SHELL__ = 'tauri';
window.addEventListener('contextmenu', (event) => {
  // Without the injected menu there is nothing to show, so leave the
  // platform's own context menu alone instead of swallowing the click.
  if (!window.__MIXCHAT_MENU__) return;
  event.preventDefault();
  window.__TAURI_INTERNALS__.invoke('show_context_menu', { x: event.clientX, y: event.clientY });
});
window.addEventListener('keydown', (event) => {
  if (event.repeat || event.altKey) return;
  const mod = event.ctrlKey || event.metaKey;
  const key = event.key.toLowerCase();
  const act = (id) => window.__TAURI_INTERNALS__.invoke('menu_action', { id });
  if (mod && event.shiftKey && key === 't') {
    event.preventDefault();
    act('twitch-login');
  } else if (mod && event.shiftKey && key === 'w') {
    event.preventDefault();
    act('close-window');
  } else if (mod && event.shiftKey && key === 'm') {
    event.preventDefault();
    act('toggle-mute');
  } else if (mod && event.shiftKey && event.key === 'Tab') {
    event.preventDefault();
    act('prev-tab');
  } else if (mod && !event.altKey && event.key === 'Tab') {
    event.preventDefault();
    act('next-tab');
  } else if (mod && !event.altKey && !event.shiftKey && key === 't') {
    event.preventDefault();
    act('new-tab');
  } else if (mod && !event.altKey && !event.shiftKey && key === 'n') {
    event.preventDefault();
    act('new-window');
  } else if (mod && !event.altKey && !event.shiftKey && key === 'w') {
    event.preventDefault();
    act('close-tab');
  } else if (mod && !event.altKey && !event.shiftKey && /^[1-9]$/.test(key)) {
    event.preventDefault();
    act('select-tab-' + key);
  } else if (mod && !event.altKey && key === 'r') {
    event.preventDefault();
    act('reload');
  } else if (mod && !event.altKey && key === 'q') {
    event.preventDefault();
    act('quit');
  } else if (!mod && event.key === 'F11') {
    event.preventDefault();
    act('fullscreen');
  } else if (!mod && event.key === 'F12') {
    event.preventDefault();
    act('devtools');
  }
});
(function () {
  const invoke = window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke;
  if (!invoke) return;
  let last = '';
  function report() {
    let value;
    const match = window.location.pathname.match(/^\/channel\/([^/]+)$/);
    if (match) {
      try {
        value = decodeURIComponent(match[1]);
      } catch {
        value = match[1];
      }
    } else {
      value = document.title || 'Mixchat';
    }
    value = String(value).replace(/[\u0000-\u001f\u007f]/g, '').trim().slice(0, 120);
    if (!value || value === last) return;
    last = value;
    invoke('tab_meta', { title: value }).catch(() => {});
  }
  function wrap(name) {
    const original = history[name];
    if (typeof original !== 'function') return;
    history[name] = function () {
      const result = original.apply(this, arguments);
      setTimeout(report, 0);
      return result;
    };
  }
  wrap('pushState');
  wrap('replaceState');
  window.addEventListener('popstate', report);
  window.addEventListener('hashchange', report);
  document.addEventListener('DOMContentLoaded', report);
  setInterval(report, 1500);
  report();
})();"#,
        "\n",
        include_str!("downloads_ui.js"),
        "\n",
        include_str!("menu_ui.js"),
    )
    .to_string()
}

/// Tauri's Linux runtime puts child webviews in a vertical `GtkBox`, where
/// absolute bounds are ignored. Configure the box packing so the strip keeps
/// its height and the active content fills the rest.
#[cfg(target_os = "linux")]
fn configure_linux_webview<R: Runtime>(webview: &Webview<R>, strip: bool) {
    let _ = webview.with_webview(move |platform| {
        use gtk::prelude::*;
        let inner = platform.inner();
        let widget: &gtk::Widget = inner.upcast_ref();
        if let Some(parent) = widget
            .parent()
            .and_then(|parent| parent.downcast::<gtk::Box>().ok())
        {
            parent.set_child_packing(widget, !strip, true, 0, gtk::PackType::Start);
        }
        widget.set_hexpand(true);
        widget.set_vexpand(!strip);
        widget.set_halign(gtk::Align::Fill);
        widget.set_valign(gtk::Align::Fill);
        widget.set_size_request(-1, if strip { TAB_BAR_HEIGHT as i32 } else { -1 });
    });
}

fn set_webview_muted<R: Runtime>(webview: &Webview<R>, muted: bool) {
    #[cfg(target_os = "linux")]
    {
        let _ = webview.with_webview(move |platform| {
            use webkit2gtk::WebViewExt;
            platform.inner().set_is_muted(muted);
        });
    }
    #[cfg(windows)]
    {
        let _ = webview.with_webview(move |platform| {
            use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
            use windows_core::Interface;
            unsafe {
                if let Ok(core) = platform.controller().CoreWebView2() {
                    if let Ok(core) = core.cast::<ICoreWebView2_8>() {
                        let _ = core.SetIsMuted(muted);
                    }
                }
            }
        });
    }
    #[cfg(target_os = "macos")]
    {
        // WKWebView has no public mute API. Suspending all media playback
        // silences the webview and resuming restores the previous state.
        let _ = webview.with_webview(move |platform| unsafe {
            let view: &objc2_web_kit::WKWebView =
                &*(platform.inner() as *const objc2_web_kit::WKWebView);
            view.setAllMediaPlaybackSuspended_completionHandler(muted, None);
        });
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = (webview, muted);
    }
}

fn add_content<R: Runtime>(
    app: &AppHandle<R>,
    window: &Window<R>,
    window_label: &str,
    tab_id: &str,
    target: Option<String>,
) -> tauri::Result<Webview<R>> {
    let label = content_label(window_label, tab_id);
    let url = content_url(app, target.as_deref());
    let origin = app_origin(app);
    let navigation_app = app.clone();
    let new_window_app = app.clone();
    let new_window_label = window_label.to_string();
    let new_window_origin = origin.clone();
    let (width, height) = content_bounds(window);

    let webview = window.add_child(
        WebviewBuilder::new(label, WebviewUrl::External(url))
            .initialization_script(content_init_script())
            .on_navigation(move |target| {
                if target.origin().ascii_serialization() == origin {
                    if target.path() == "/desktop/twitch-login" {
                        let _ = crate::open_twitch_login(&navigation_app);
                        return false;
                    }
                    if target.path() == "/desktop/twitch-help" {
                        let _ = crate::open_twitch_help(&navigation_app);
                        return false;
                    }
                    if target.path() == "/desktop/twitch-close" {
                        crate::close_twitch_login_windows(&navigation_app);
                        return false;
                    }
                    if let Some(activation) = crate::twitch_activation_target(target) {
                        let _ = crate::open_twitch_activation(&navigation_app, activation);
                        return false;
                    }
                    return true;
                }
                crate::open_external_url(&navigation_app, target);
                false
            })
            .on_new_window(move |target, _features| {
                if target.origin().ascii_serialization() == new_window_origin {
                    if target.path() == "/desktop/twitch-login" {
                        let _ = crate::open_twitch_login(&new_window_app);
                        return NewWindowResponse::Deny;
                    }
                    if target.path() == "/desktop/twitch-help" {
                        let _ = crate::open_twitch_help(&new_window_app);
                        return NewWindowResponse::Deny;
                    }
                    if target.path() == "/desktop/twitch-close" {
                        crate::close_twitch_login_windows(&new_window_app);
                        return NewWindowResponse::Deny;
                    }
                    if let Some(activation) = crate::twitch_activation_target(&target) {
                        let _ = crate::open_twitch_activation(&new_window_app, activation);
                        return NewWindowResponse::Deny;
                    }
                    create_tab(&new_window_app, &new_window_label, Some(target.to_string()));
                    return NewWindowResponse::Deny;
                }
                crate::open_external_url(&new_window_app, &target);
                NewWindowResponse::Deny
            }),
        LogicalPosition::new(0.0, TAB_BAR_HEIGHT),
        LogicalSize::new(width, height),
    )?;
    #[cfg(target_os = "linux")]
    configure_linux_webview(&webview, false);
    Ok(webview)
}

/// Create a new workspace window. Safe to call from commands and event handlers:
/// window and webview creation happens on a fresh thread to avoid the WebView2
/// deadlock documented by Tauri.
pub(crate) fn create_window<R: Runtime>(app: &AppHandle<R>, target: Option<String>) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(error) = build_window(&app, target) {
            report_failure(&app, &format!("[tabs] failed to open window: {error}"));
            if first_window(&app).is_none() {
                app.dialog()
                    .message(format!("Mixchat could not open its window.\n\n{error}"))
                    .kind(MessageDialogKind::Error)
                    .title("Mixchat failed to start")
                    .blocking_show();
                app.exit(1);
            }
        }
    });
}

fn report_failure<R: Runtime>(app: &AppHandle<R>, message: &str) {
    if let Some(state) = app.try_state::<Arc<ServerState>>() {
        state.log_error(message);
    }
    eprintln!("{message}");
}

fn build_window<R: Runtime>(app: &AppHandle<R>, target: Option<String>) -> Result<(), String> {
    let manager = tabs(app).ok_or_else(|| "tabs state is not ready".to_string())?;
    let window_label = manager.allocate_window_label();
    let window = match Window::builder(app, &window_label)
        .title("Mixchat")
        .theme(Some(tauri::Theme::Dark))
        .inner_size(1600.0, 900.0)
        .min_inner_size(900.0, 600.0)
        .build()
    {
        Ok(window) => window,
        Err(error) => {
            manager.release_window_label(&window_label);
            return Err(error.to_string());
        }
    };

    let tab_id = manager.allocate_tab_id();
    let webview = content_label(&window_label, &tab_id);
    manager.insert_workspace(
        &window_label,
        Tab {
            id: tab_id.clone(),
            webview,
            title: "Mixchat".to_string(),
            muted: false,
        },
    );

    let (width, _) = content_bounds(&window);
    let strip = WebviewBuilder::new(
        strip_label(&window_label),
        WebviewUrl::CustomProtocol(strip_url()),
    )
    .on_navigation(|target| is_strip_url(target))
    .on_new_window(|_, _| NewWindowResponse::Deny);

    match window.add_child(
        strip,
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(width, TAB_BAR_HEIGHT),
    ) {
        Ok(_webview) => {
            #[cfg(target_os = "linux")]
            configure_linux_webview(&_webview, true);
        }
        Err(error) => {
            manager.remove_workspace(&window_label);
            let _ = window.close();
            return Err(error.to_string());
        }
    }

    if let Err(error) = add_content(app, &window, &window_label, &tab_id, target) {
        manager.remove_workspace(&window_label);
        let _ = window.close();
        return Err(error.to_string());
    }

    let resize_app = app.clone();
    let resize_window = window_label.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
            layout_workspace(&resize_app, &resize_window);
        }
        WindowEvent::Destroyed => {
            if let Some(manager) = resize_app.try_state::<Tabs>() {
                manager.remove_workspace(&resize_window);
            }
        }
        _ => {}
    });

    let _ = window.set_theme(Some(tauri::Theme::Dark));
    layout_workspace(app, &window_label);
    if let Some(webview) = app.get_webview(&content_label(&window_label, &tab_id)) {
        let _ = window.set_focus();
        let _ = webview.set_focus();
    }
    sync_workspace(app, &window_label);
    Ok(())
}

fn layout_workspace<R: Runtime>(app: &AppHandle<R>, window_label: &str) {
    let Some(window) = app.get_window(window_label) else {
        return;
    };
    let (width, height) = content_bounds(&window);
    if let Some(strip) = app.get_webview(&strip_label(window_label)) {
        let _ = strip.set_position(LogicalPosition::new(0.0, 0.0));
        let _ = strip.set_size(LogicalSize::new(width, TAB_BAR_HEIGHT));
    }
    let Some(manager) = tabs(app) else {
        return;
    };
    let Some(active) = manager.active_webview(window_label) else {
        return;
    };
    if let Some(webview) = app.get_webview(&active) {
        let _ = webview.set_position(LogicalPosition::new(0.0, TAB_BAR_HEIGHT));
        let _ = webview.set_size(LogicalSize::new(width, height));
    }
}

fn sync_workspace<R: Runtime>(app: &AppHandle<R>, window_label: &str) {
    let Some(manager) = tabs(app) else {
        return;
    };
    let Some(view) = manager.view(window_label) else {
        return;
    };
    if let Some(window) = app.get_window(window_label) {
        let title = view
            .tabs
            .iter()
            .find(|tab| tab.id == view.active)
            .map(|tab| tab.title.as_str())
            .unwrap_or("Mixchat");
        let _ = window.set_title(&window_title(title));
    }
    let Ok(json) = serde_json::to_string(&view) else {
        return;
    };
    if let Some(strip) = app.get_webview(&strip_label(window_label)) {
        let _ = strip.eval(format!(
            "window.__MIXCHAT_TABS__&&window.__MIXCHAT_TABS__.render({json});"
        ));
    }
}

pub(crate) fn select_tab<R: Runtime>(app: &AppHandle<R>, window_label: &str, tab_id: &str) {
    let Some(manager) = tabs(app) else {
        return;
    };
    let (previous, selected) = {
        let mut state = manager.lock();
        let Some(workspace) = state.workspaces.get_mut(window_label) else {
            return;
        };
        let Some(tab) = workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.webview.clone())
        else {
            return;
        };
        let previous = workspace.active.clone();
        workspace.active = tab_id.to_string();
        let previous = workspace
            .tabs
            .iter()
            .find(|tab| tab.id == previous)
            .map(|tab| tab.webview.clone());
        (previous, tab)
    };

    if let Some(previous) = previous.as_deref() {
        if previous != selected {
            if let Some(webview) = app.get_webview(previous) {
                let _ = webview.hide();
            }
        }
    }
    if let Some(webview) = app.get_webview(&selected) {
        let _ = webview.show();
        let _ = webview.set_focus();
    }
    layout_workspace(app, window_label);
    sync_workspace(app, window_label);
}

fn build_tab<R: Runtime>(
    app: &AppHandle<R>,
    window_label: &str,
    target: Option<String>,
) -> Result<(), String> {
    let Some(manager) = tabs(app) else {
        return Err("tabs state is not ready".to_string());
    };
    let Some(window) = app.get_window(window_label) else {
        return Err("window is gone".to_string());
    };
    let tab_id = manager.allocate_tab_id();
    let label = content_label(window_label, &tab_id);
    let previous = manager
        .reserve_tab(
            window_label,
            Tab {
                id: tab_id.clone(),
                webview: label,
                title: "Mixchat".to_string(),
                muted: false,
            },
        )
        .ok_or_else(|| "window is gone".to_string())?;

    if let Some(label) = manager.webview_label(window_label, &previous) {
        if let Some(webview) = app.get_webview(&label) {
            let _ = webview.hide();
        }
    }

    match add_content(app, &window, window_label, &tab_id, target) {
        Ok(webview) => {
            if manager.active_id(window_label).as_deref() == Some(tab_id.as_str()) {
                let _ = webview.show();
                let _ = webview.set_focus();
            } else {
                let _ = webview.hide();
            }
            layout_workspace(app, window_label);
            sync_workspace(app, window_label);
            Ok(())
        }
        Err(error) => {
            if let Some(restored) = manager.rollback_tab(window_label, &tab_id, &previous) {
                if let Some(label) = manager.webview_label(window_label, &restored) {
                    if let Some(webview) = app.get_webview(&label) {
                        let _ = webview.show();
                        let _ = webview.set_focus();
                    }
                }
            }
            sync_workspace(app, window_label);
            Err(error.to_string())
        }
    }
}

pub(crate) fn create_tab<R: Runtime>(
    app: &AppHandle<R>,
    window_label: &str,
    target: Option<String>,
) {
    let app = app.clone();
    let window_label = window_label.to_string();
    std::thread::spawn(move || {
        if let Err(error) = build_tab(&app, &window_label, target) {
            report_failure(&app, &format!("[tabs] failed to open tab: {error}"));
        }
    });
}

pub(crate) fn close_tab<R: Runtime>(app: &AppHandle<R>, window_label: &str, tab_id: &str) {
    let Some(manager) = tabs(app) else {
        return;
    };
    match manager.close_tab(window_label, tab_id) {
        CloseOutcome::Missing => {}
        CloseOutcome::CloseWindow => {
            if let Some(window) = app.get_window(window_label) {
                let _ = window.close();
            }
        }
        CloseOutcome::Closed { next, removed } => {
            if let Some(webview) = app.get_webview(&removed) {
                let _ = webview.close();
            }
            select_tab(app, window_label, &next);
        }
    }
}

pub(crate) fn toggle_mute<R: Runtime>(app: &AppHandle<R>, window_label: &str, tab_id: &str) {
    let Some(manager) = tabs(app) else {
        return;
    };
    let Some((label, muted)) = manager.toggle_mute(window_label, tab_id) else {
        return;
    };
    if let Some(webview) = app.get_webview(&label) {
        set_webview_muted(&webview, muted);
    }
    sync_workspace(app, window_label);
}

pub(crate) fn cycle_tab<R: Runtime>(app: &AppHandle<R>, window_label: &str, step: isize) {
    let Some(manager) = tabs(app) else {
        return;
    };
    let Some(tab_id) = manager.cycle(window_label, step) else {
        return;
    };
    select_tab(app, window_label, &tab_id);
}

pub(crate) fn select_index<R: Runtime>(app: &AppHandle<R>, window_label: &str, index: usize) {
    let Some(manager) = tabs(app) else {
        return;
    };
    let Some(tab_id) = manager.select_index(window_label, index) else {
        return;
    };
    select_tab(app, window_label, &tab_id);
}

fn window_for_webview<R: Runtime>(app: &AppHandle<R>, label: &str) -> Option<String> {
    tabs(app).and_then(|manager| manager.window_for_shell(label))
}

fn tab_for_webview<R: Runtime>(app: &AppHandle<R>, label: &str) -> Option<(String, String)> {
    tabs(app).and_then(|manager| manager.workspace_for_content(label))
}

pub(crate) fn new_tab_for_webview<R: Runtime>(app: &AppHandle<R>, webview_label: &str) {
    if let Some(window) = window_for_webview(app, webview_label) {
        create_tab(app, &window, None);
    }
}

/// Active content webview label for the workspace a shell (tab strip) label
/// belongs to. Used to aim menu actions from the strip's menu button.
pub(crate) fn active_content_label<R: Runtime>(
    app: &AppHandle<R>,
    shell_label: &str,
) -> Option<String> {
    let manager = tabs(app)?;
    let window = manager.window_for_shell(shell_label)?;
    manager.active_webview(&window)
}

pub(crate) fn close_active_tab<R: Runtime>(app: &AppHandle<R>, webview_label: &str) {
    if let Some((window, tab_id)) = tab_for_webview(app, webview_label) {
        close_tab(app, &window, &tab_id);
    }
}

pub(crate) fn close_window_for_webview<R: Runtime>(app: &AppHandle<R>, webview_label: &str) {
    if let Some(window) = window_for_webview(app, webview_label) {
        if let Some(window) = app.get_window(&window) {
            let _ = window.close();
        }
    }
}

pub(crate) fn toggle_active_mute<R: Runtime>(app: &AppHandle<R>, webview_label: &str) {
    if let Some((window, tab_id)) = tab_for_webview(app, webview_label) {
        toggle_mute(app, &window, &tab_id);
    }
}

pub(crate) fn cycle_for_webview<R: Runtime>(app: &AppHandle<R>, webview_label: &str, step: isize) {
    if let Some(window) = window_for_webview(app, webview_label) {
        cycle_tab(app, &window, step);
    }
}

pub(crate) fn select_index_for_webview<R: Runtime>(
    app: &AppHandle<R>,
    webview_label: &str,
    index: usize,
) {
    if let Some(window) = window_for_webview(app, webview_label) {
        select_index(app, &window, index);
    }
}

pub(crate) fn duplicate_in_new_window<R: Runtime>(app: &AppHandle<R>, webview_label: &str) {
    let target = app
        .get_webview(webview_label)
        .and_then(|webview| webview.url().ok())
        .map(|url| url.to_string());
    create_window(app, target);
}

pub(crate) fn reload_all<R: Runtime>(app: &AppHandle<R>) {
    let Some(manager) = tabs(app) else {
        return;
    };
    for label in manager.content_labels() {
        if let Some(webview) = app.get_webview(&label) {
            let _ = webview.reload();
        }
    }
}

/// The active content webview of the focused workspace window, else of the
/// first workspace in a stable order.
pub(crate) fn first_content_webview<R: Runtime>(app: &AppHandle<R>) -> Option<Webview<R>> {
    let manager = tabs(app)?;
    let mut order = Vec::new();
    if let Some(focused) = app.get_focused_window() {
        order.push(focused.label().to_string());
    }
    order.extend(manager.workspace_labels());
    for window in order {
        if let Some(label) = manager.active_webview(&window) {
            if let Some(webview) = app.get_webview(&label) {
                return Some(webview);
            }
        }
    }
    None
}

pub(crate) fn first_window<R: Runtime>(app: &AppHandle<R>) -> Option<Window<R>> {
    let manager = tabs(app);
    let mut labels: Vec<String> = manager
        .as_ref()
        .map(|manager| manager.workspace_labels())
        .unwrap_or_default();
    // Move the focused workspace to the front, but only among workspace labels
    // so an about or Twitch child window can never be picked here.
    if let Some(focused) = app.get_focused_window() {
        let focused_label = focused.label().to_string();
        if let Some(index) = labels.iter().position(|label| *label == focused_label) {
            labels.remove(index);
            labels.insert(0, focused_label);
        }
    }
    for label in &labels {
        if let Some(window) = app.get_window(label) {
            return Some(window);
        }
    }
    if manager.is_some() {
        return None;
    }
    // Tabs state is not managed yet (startup or teardown): fall back to any
    // window that is not an about or Twitch child, in a stable order.
    let mut windows: Vec<_> = app
        .windows()
        .into_iter()
        .filter(|(label, _)| label != "about" && !label.starts_with("twitch-"))
        .collect();
    windows.sort_by(|(a, _), (b, _)| a.cmp(b));
    windows.into_iter().next().map(|(_label, window)| window)
}

pub(crate) fn focus_first_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = first_window(app) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub(crate) fn tab_strip_response(
    request: &tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    match request.uri().path() {
        "/" | "/index.html" => tauri::http::Response::builder()
            .header(
                tauri::http::header::CONTENT_TYPE,
                "text/html; charset=utf-8",
            )
            .header(tauri::http::header::CACHE_CONTROL, "no-store")
            .header(
                "Content-Security-Policy",
                "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data:; connect-src ipc: http://ipc.localhost",
            )
            .header("X-Content-Type-Options", "nosniff")
            .body(TAB_STRIP_HTML.as_bytes().to_vec())
            .expect("static tab strip response"),
        "/favicon.ico" => tauri::http::Response::builder()
            .status(tauri::http::StatusCode::NO_CONTENT)
            .body(Vec::new())
            .expect("static favicon response"),
        _ => tauri::http::Response::builder()
            .status(tauri::http::StatusCode::NOT_FOUND)
            .body(Vec::new())
            .expect("static not found response"),
    }
}

#[tauri::command]
pub fn tabs_ready<R: Runtime>(app: AppHandle<R>, webview: Webview<R>) {
    if let Some(window) = window_for_webview(&app, webview.label()) {
        sync_workspace(&app, &window);
    }
}

#[tauri::command]
pub fn tabs_select<R: Runtime>(app: AppHandle<R>, webview: Webview<R>, id: String) {
    if let Some(window) = window_for_webview(&app, webview.label()) {
        select_tab(&app, &window, &id);
    }
}

#[tauri::command]
pub fn tabs_close<R: Runtime>(app: AppHandle<R>, webview: Webview<R>, id: String) {
    if let Some(window) = window_for_webview(&app, webview.label()) {
        close_tab(&app, &window, &id);
    }
}

#[tauri::command]
pub fn tabs_new<R: Runtime>(app: AppHandle<R>, webview: Webview<R>) {
    if let Some(window) = window_for_webview(&app, webview.label()) {
        create_tab(&app, &window, None);
    }
}

#[tauri::command]
pub fn tabs_new_window<R: Runtime>(app: AppHandle<R>) {
    create_window(&app, None);
}

#[tauri::command]
pub fn tabs_toggle_mute<R: Runtime>(app: AppHandle<R>, webview: Webview<R>, id: String) {
    if let Some(window) = window_for_webview(&app, webview.label()) {
        toggle_mute(&app, &window, &id);
    }
}

#[tauri::command]
pub fn tabs_cycle<R: Runtime>(app: AppHandle<R>, webview: Webview<R>, step: i32) {
    if let Some(window) = window_for_webview(&app, webview.label()) {
        cycle_tab(&app, &window, step.signum() as isize);
    }
}

#[tauri::command]
pub fn tabs_select_index<R: Runtime>(app: AppHandle<R>, webview: Webview<R>, index: usize) {
    if let Some(window) = window_for_webview(&app, webview.label()) {
        select_index(&app, &window, index);
    }
}

#[tauri::command]
pub fn tab_meta<R: Runtime>(app: AppHandle<R>, webview: Webview<R>, title: String) {
    let clean = clean_title(&title);
    let Some((window, tab_id)) = tab_for_webview(&app, webview.label()) else {
        return;
    };
    let Some(manager) = tabs(&app) else {
        return;
    };
    if manager.set_title(&window, &tab_id, &clean) {
        sync_workspace(&app, &window);
    }
}

const TAB_STRIP_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>Mixchat tabs</title>
<style>
  :root { color-scheme: dark; }
  * { box-sizing: border-box; }
  html, body { margin: 0; height: 100%; overflow: hidden; background: #0b0b0b; }
  body {
    font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
    color: #e5e5e5;
    user-select: none;
  }
  #strip {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 100%;
    padding: 4px 6px;
    background: #0b0b0b;
    border-bottom: 1px solid #262626;
  }
  #tabs {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: 1;
    min-width: 0;
    overflow-x: auto;
    scrollbar-width: none;
  }
  #tabs::-webkit-scrollbar { display: none; }
  .tab {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 1 1 auto;
    min-width: 110px;
    max-width: 230px;
    height: 28px;
    padding: 0 4px 0 10px;
    border: 1px solid transparent;
    border-radius: 6px;
    background: #121212;
    color: #a3a3a3;
    cursor: default;
    overflow: hidden;
  }
  .tab:hover { background: #181818; color: #d4d4d4; }
  .tab.active {
    background: #1e1e1e;
    border-color: #2e2e2e;
    color: #f5f5f5;
  }
  .tab .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
  }
  .tab.muted .title { opacity: 0.6; }
  .icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: inherit;
    cursor: pointer;
    opacity: 0.55;
  }
  .tab:hover .icon, .tab.active .icon, .tab.muted .icon { opacity: 1; }
  .icon:hover { background: rgba(255, 255, 255, 0.08); }
  .tab .close { visibility: hidden; }
  .tab:hover .close, .tab.active .close { visibility: visible; }
  .tab .mute[data-muted="true"] { color: #fbbf24; }
  .bar-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 28px;
    height: 28px;
    padding: 0;
    border: 1px solid transparent;
    border-radius: 6px;
    background: transparent;
    color: #a3a3a3;
    cursor: pointer;
  }
  .bar-button:hover { background: #181818; color: #f5f5f5; }
  .tab:focus-visible, .icon:focus-visible, .bar-button:focus-visible {
    outline: 2px solid #818cf8;
    outline-offset: 1px;
  }
  svg { display: block; width: 14px; height: 14px; }
</style>
</head>
<body>
<div id="strip">
  <button id="menu-button" class="bar-button" type="button" title="Menu" aria-label="Menu">
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M2.5 4.5h11M2.5 8h11M2.5 11.5h11"/></svg>
  </button>
  <div id="tabs" role="tablist" aria-label="Stream tabs">
    <button id="add-tab" class="bar-button" type="button" title="New tab (Ctrl+T)" aria-label="New tab">
      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M8 3v10M3 8h10"/></svg>
    </button>
  </div>
  <button id="new-window" class="bar-button" type="button" title="New window (Ctrl+N)" aria-label="New window">
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="2" y="4" width="9" height="9" rx="1.5"/><path d="M5 4V2.5h9V11h-1.5"/></svg>
  </button>
</div>
<script>
(function () {
  const bridge = window.__TAURI_INTERNALS__;
  const invoke = (command, args) => bridge.invoke(command, args || {});
  const tabsEl = document.getElementById('tabs');
  const addTab = document.getElementById('add-tab');
  const icons = {
    sound: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h2.5L9 3v10L5.5 10H3z"/><path d="M11.5 5.5a3.5 3.5 0 0 1 0 5"/></svg>',
    muted: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h2.5L9 3v10L5.5 10H3z"/><path d="M11 6l3 4M14 6l-3 4"/></svg>',
    close: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M4 4l8 8M12 4l-8 8"/></svg>'
  };
  let current = { tabs: [], active: '' };

  function button(className, label, markup) {
    const el = document.createElement('button');
    el.type = 'button';
    el.className = className;
    el.title = label;
    el.setAttribute('aria-label', label);
    el.innerHTML = markup;
    return el;
  }

  function render(next) {
    if (next) current = next;
    // Rebuilding the DOM would otherwise drop keyboard focus on every title
    // update, so remember the focused control and put it back afterwards.
    const focused = document.activeElement;
    const focusedTab = focused && focused.closest ? focused.closest('.tab') : null;
    const focusedTabId = focusedTab ? focusedTab.dataset.id : '';
    const focusedRole = focused && focused.dataset ? focused.dataset.role || '' : '';
    // The add button lives inside the container that gets cleared below, so
    // clearing it drops focus and the button has to be focused again.
    const focusedAddTab = focused === addTab;

    tabsEl.textContent = '';
    for (const tab of current.tabs) {
      const el = document.createElement('div');
      const active = tab.id === current.active;
      el.className = 'tab' + (active ? ' active' : '') + (tab.muted ? ' muted' : '');
      el.dataset.id = tab.id;
      el.setAttribute('role', 'tab');
      el.setAttribute('aria-selected', active ? 'true' : 'false');
      // Roving tabindex: one tab in the Tab order, arrow keys move between.
      el.tabIndex = active ? 0 : -1;
      el.addEventListener('focus', () => {
        for (const other of tabsEl.querySelectorAll('.tab')) {
          other.tabIndex = other === el ? 0 : -1;
        }
      });

      const title = document.createElement('span');
      title.className = 'title';
      title.textContent = tab.title || 'Mixchat';
      title.title = tab.title || 'Mixchat';

      const mute = button(
        'icon mute',
        tab.muted ? 'Unmute tab' : 'Mute tab',
        tab.muted ? icons.muted : icons.sound
      );
      mute.dataset.role = 'mute';
      mute.dataset.muted = tab.muted ? 'true' : 'false';
      mute.addEventListener('click', (event) => {
        event.stopPropagation();
        invoke('tabs_toggle_mute', { id: tab.id });
      });

      const close = button('icon close', 'Close tab', icons.close);
      close.dataset.role = 'close';
      close.addEventListener('click', (event) => {
        event.stopPropagation();
        invoke('tabs_close', { id: tab.id });
      });

      el.addEventListener('click', () => invoke('tabs_select', { id: tab.id }));
      el.addEventListener('auxclick', (event) => {
        if (event.button === 1) invoke('tabs_close', { id: tab.id });
      });
      el.addEventListener('keydown', (event) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault();
          invoke('tabs_select', { id: tab.id });
        } else if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') {
          event.preventDefault();
          focus_neighbour(tab.id, event.key === 'ArrowRight' ? 1 : -1);
        }
      });

      el.append(title, mute, close);
      tabsEl.append(el);
    }
    tabsEl.append(addTab);
    if (focusedAddTab) {
      addTab.focus();
    } else if (focusedTabId) {
      for (const el of tabsEl.children) {
        if (el.dataset.id !== focusedTabId) continue;
        const target =
          (focusedRole && el.querySelector('[data-role="' + focusedRole + '"]')) || el;
        // focus() already brings the restored control into view; scrolling to
        // the active tab here would fight it.
        target.focus();
        break;
      }
    } else {
      const active = tabsEl.querySelector('.tab.active');
      if (active) active.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    }
  }

  function focus_neighbour(fromId, step) {
    const ids = current.tabs.map((tab) => tab.id);
    if (ids.length === 0) return;
    const index = ids.indexOf(fromId);
    if (index < 0) return;
    const next = ids[(index + step + ids.length) % ids.length];
    for (const el of tabsEl.children) {
      if (el.dataset.id === next) {
        el.focus();
        return;
      }
    }
  }

  window.__MIXCHAT_TABS__ = { render };

  document.getElementById('menu-button').addEventListener('click', () => invoke('show_menu'));
  document.getElementById('add-tab').addEventListener('click', () => invoke('tabs_new'));
  document.getElementById('new-window').addEventListener('click', () => invoke('tabs_new_window'));
  document.addEventListener('contextmenu', (event) => event.preventDefault());
  document.addEventListener('keydown', (event) => {
    const mod = event.ctrlKey || event.metaKey;
    if (!mod || event.repeat || event.altKey) return;
    const key = event.key.toLowerCase();
    if (event.shiftKey && event.key === 'Tab') {
      event.preventDefault();
      invoke('tabs_cycle', { step: -1 });
    } else if (event.key === 'Tab') {
      event.preventDefault();
      invoke('tabs_cycle', { step: 1 });
    } else if (!event.shiftKey && key === 't') {
      event.preventDefault();
      invoke('tabs_new');
    } else if (!event.shiftKey && key === 'n') {
      event.preventDefault();
      invoke('tabs_new_window');
    } else if (event.shiftKey && key === 'w') {
      event.preventDefault();
      invoke('menu_action', { id: 'close-window' });
    } else if (!event.shiftKey && key === 'w' && current.active) {
      event.preventDefault();
      invoke('tabs_close', { id: current.active });
    } else if (event.shiftKey && key === 'm' && current.active) {
      event.preventDefault();
      invoke('tabs_toggle_mute', { id: current.active });
    } else if (!event.shiftKey && /^[1-9]$/.test(key)) {
      event.preventDefault();
      invoke('tabs_select_index', { index: Number(key) });
    }
  });

  invoke('tabs_ready').catch(() => {});
})();
</script>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_titles() {
        assert_eq!(clean_title("  hello\u{0} world  "), "hello world");
        assert_eq!(clean_title("\u{202e}spoiled"), "spoiled");
        assert_eq!(clean_title("   "), "Mixchat");
        assert_eq!(clean_title(&"a".repeat(200)).len(), MAX_TITLE);
    }

    #[test]
    fn window_titles_drop_redundant_brand() {
        assert_eq!(window_title("Mixchat"), "Mixchat");
        assert_eq!(window_title(""), "Mixchat");
        assert_eq!(window_title("somechannel"), "Mixchat - somechannel");
    }

    #[test]
    fn parses_strip_labels() {
        let tabs = Tabs::new();
        tabs.insert_workspace(
            "main",
            Tab {
                id: "tab-1".to_string(),
                webview: "content-main-tab-1".to_string(),
                title: "Mixchat".to_string(),
                muted: false,
            },
        );
        assert_eq!(tabs.window_for_shell("tabs-main"), Some("main".to_string()));
        assert_eq!(
            tabs.window_for_shell("content-main-tab-1"),
            Some("main".to_string())
        );
        assert_eq!(tabs.window_for_shell("tabs-missing"), None);
    }

    #[test]
    fn updates_titles_and_mute_state() {
        let tabs = Tabs::new();
        tabs.insert_workspace(
            "main",
            Tab {
                id: "tab-1".to_string(),
                webview: "w1".to_string(),
                title: "Mixchat".to_string(),
                muted: false,
            },
        );
        assert!(tabs.set_title("main", "tab-1", "somechannel"));
        assert!(!tabs.set_title("main", "tab-1", "somechannel"));
        assert_eq!(
            tabs.view("main").unwrap().tabs[0].title,
            "somechannel".to_string()
        );

        let (label, muted) = tabs.toggle_mute("main", "tab-1").unwrap();
        assert_eq!(label, "w1");
        assert!(muted);
        assert!(tabs.view("main").unwrap().tabs[0].muted);
    }

    #[test]
    fn prefers_main_window_for_updates_and_single_instance() {
        let tabs = Tabs::new();
        tabs.insert_workspace(
            "window-2",
            Tab {
                id: "tab-2".to_string(),
                webview: "w2".to_string(),
                title: "two".to_string(),
                muted: false,
            },
        );
        tabs.insert_workspace(
            "main",
            Tab {
                id: "tab-1".to_string(),
                webview: "w1".to_string(),
                title: "one".to_string(),
                muted: false,
            },
        );
        assert_eq!(
            tabs.workspace_labels().first().map(String::as_str),
            Some("main")
        );
    }

    #[test]
    fn closing_active_tab_picks_a_neighbour() {
        let tabs = Tabs::new();
        tabs.insert_workspace(
            "main",
            Tab {
                id: "tab-1".to_string(),
                webview: "w1".to_string(),
                title: "one".to_string(),
                muted: false,
            },
        );
        tabs.reserve_tab(
            "main",
            Tab {
                id: "tab-2".to_string(),
                webview: "w2".to_string(),
                title: "two".to_string(),
                muted: false,
            },
        );
        tabs.reserve_tab(
            "main",
            Tab {
                id: "tab-3".to_string(),
                webview: "w3".to_string(),
                title: "three".to_string(),
                muted: false,
            },
        );
        tabs.lock().workspaces.get_mut("main").unwrap().active = "tab-2".to_string();
        match tabs.close_tab("main", "tab-2") {
            CloseOutcome::Closed { next, removed } => {
                assert_eq!(next, "tab-3");
                assert_eq!(removed, "w2");
            }
            _ => panic!("expected a closed tab"),
        }
    }

    #[test]
    fn cycle_wraps_both_ways() {
        let tabs = Tabs::new();
        tabs.insert_workspace(
            "main",
            Tab {
                id: "tab-1".to_string(),
                webview: "w1".to_string(),
                title: "one".to_string(),
                muted: false,
            },
        );
        tabs.reserve_tab(
            "main",
            Tab {
                id: "tab-2".to_string(),
                webview: "w2".to_string(),
                title: "two".to_string(),
                muted: false,
            },
        );
        assert_eq!(tabs.cycle("main", 1), Some("tab-1".to_string()));
        tabs.lock().workspaces.get_mut("main").unwrap().active = "tab-1".to_string();
        assert_eq!(tabs.cycle("main", -1), Some("tab-2".to_string()));
    }

    #[test]
    fn stale_close_id_leaves_a_single_tab_window_open() {
        let tabs = Tabs::new();
        tabs.insert_workspace(
            "main",
            Tab {
                id: "tab-1".to_string(),
                webview: "w1".to_string(),
                title: "one".to_string(),
                muted: false,
            },
        );
        // An id the strip has not re-rendered away yet must not tear down a
        // window whose only tab is a different one.
        assert!(matches!(
            tabs.close_tab("main", "tab-gone"),
            CloseOutcome::Missing
        ));
        assert!(matches!(
            tabs.close_tab("main", "tab-1"),
            CloseOutcome::CloseWindow
        ));
    }

    #[test]
    fn select_index_clamps_to_the_last_tab() {
        let tabs = Tabs::new();
        tabs.insert_workspace(
            "main",
            Tab {
                id: "tab-1".to_string(),
                webview: "w1".to_string(),
                title: "one".to_string(),
                muted: false,
            },
        );
        tabs.reserve_tab(
            "main",
            Tab {
                id: "tab-2".to_string(),
                webview: "w2".to_string(),
                title: "two".to_string(),
                muted: false,
            },
        );
        tabs.reserve_tab(
            "main",
            Tab {
                id: "tab-3".to_string(),
                webview: "w3".to_string(),
                title: "three".to_string(),
                muted: false,
            },
        );
        assert_eq!(tabs.select_index("main", 1), Some("tab-1".to_string()));
        assert_eq!(tabs.select_index("main", 3), Some("tab-3".to_string()));
        assert_eq!(tabs.select_index("main", 9), Some("tab-3".to_string()));
        assert_eq!(tabs.select_index("main", 0), None);
    }

    #[test]
    fn rollback_restores_the_previously_active_tab() {
        let tabs = Tabs::new();
        tabs.insert_workspace(
            "main",
            Tab {
                id: "tab-1".to_string(),
                webview: "w1".to_string(),
                title: "one".to_string(),
                muted: false,
            },
        );
        tabs.reserve_tab(
            "main",
            Tab {
                id: "tab-2".to_string(),
                webview: "w2".to_string(),
                title: "two".to_string(),
                muted: false,
            },
        );
        tabs.reserve_tab(
            "main",
            Tab {
                id: "tab-3".to_string(),
                webview: "w3".to_string(),
                title: "three".to_string(),
                muted: false,
            },
        );
        // tab-3 fails to build; tab-1 was active before it, so tab-1 must be
        // active again rather than whatever happened to be last in the vec.
        let restored = tabs.rollback_tab("main", "tab-3", "tab-1");
        assert_eq!(restored, Some("tab-1".to_string()));
        assert_eq!(tabs.view("main").unwrap().active, "tab-1".to_string());
        assert_eq!(tabs.view("main").unwrap().tabs.len(), 2);
    }
}
