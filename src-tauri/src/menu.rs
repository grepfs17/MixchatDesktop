use std::sync::Mutex;

use tauri::menu::{ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::{AppHandle, Manager, Runtime, Webview};
use tauri_plugin_opener::OpenerExt;

use crate::server::ServerState;

/// The webview that opened the most recent context menu. Menu events do not
/// carry a target, so they use this to act on the window the user right-clicked.
#[derive(Default)]
pub struct ContextTarget(Mutex<Option<String>>);

/// Flat menu shown on right-click. Submenus are inlined because popup menus
/// render one level at a time.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let copy = PredefinedMenuItem::copy(app, Some("Copy"))?;
    let cut = PredefinedMenuItem::cut(app, Some("Cut"))?;
    let paste = PredefinedMenuItem::paste(app, Some("Paste"))?;
    let select_all = PredefinedMenuItem::select_all(app, Some("Select All"))?;

    let version = app.package_info().version.to_string();
    let new_window = MenuItem::with_id(app, "new-window", "New window", true, Some("CmdOrCtrl+N"))?;
    let new_tab = MenuItem::with_id(app, "new-tab", "New tab", true, Some("CmdOrCtrl+T"))?;
    let open_stream_window = MenuItem::with_id(
        app,
        "open-stream-window",
        "Open stream in new window",
        true,
        None::<&str>,
    )?;
    let close_tab = MenuItem::with_id(app, "close-tab", "Close tab", true, Some("CmdOrCtrl+W"))?;
    let close_window = MenuItem::with_id(
        app,
        "close-window",
        "Close window",
        true,
        Some("CmdOrCtrl+Shift+W"),
    )?;
    let toggle_mute = MenuItem::with_id(
        app,
        "toggle-mute",
        "Mute or unmute tab",
        true,
        Some("CmdOrCtrl+Shift+M"),
    )?;
    let about = MenuItem::with_id(
        app,
        "about",
        format!("About Mixchat ({version})"),
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, Some("CmdOrCtrl+Q"))?;
    let reload = MenuItem::with_id(app, "reload", "Reload", true, Some("CmdOrCtrl+R"))?;
    #[cfg(debug_assertions)]
    let devtools = MenuItem::with_id(app, "devtools", "Toggle DevTools", true, Some("F12"))?;
    let fullscreen = MenuItem::with_id(app, "fullscreen", "Toggle Fullscreen", true, Some("F11"))?;
    let restart = MenuItem::with_id(app, "restart-server", "Restart server", true, None::<&str>)?;
    let logs = MenuItem::with_id(app, "open-logs", "Open logs", true, None::<&str>)?;
    let check_updates = MenuItem::with_id(
        app,
        "check-updates",
        "Check for updates",
        true,
        None::<&str>,
    )?;
    let twitch_login = MenuItem::with_id(
        app,
        "twitch-login",
        "Log in to Twitch player",
        true,
        Some("CmdOrCtrl+Shift+T"),
    )?;
    let twitch_help =
        MenuItem::with_id(app, "twitch-help", "Twitch login help", true, None::<&str>)?;

    let menu = Menu::new(app)?;
    menu.append(&copy)?;
    menu.append(&cut)?;
    menu.append(&paste)?;
    menu.append(&select_all)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&new_window)?;
    menu.append(&new_tab)?;
    menu.append(&open_stream_window)?;
    menu.append(&close_tab)?;
    menu.append(&close_window)?;
    menu.append(&toggle_mute)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&reload)?;
    #[cfg(debug_assertions)]
    menu.append(&devtools)?;
    menu.append(&fullscreen)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&twitch_login)?;
    menu.append(&twitch_help)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&restart)?;
    menu.append(&logs)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&check_updates)?;
    menu.append(&about)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&quit)?;
    Ok(menu)
}

pub fn handle<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let target = app
        .try_state::<ContextTarget>()
        .and_then(|state| state.0.lock().ok().and_then(|target| target.clone()));
    handle_id(app, target.as_deref(), event.id().as_ref());
}

fn target_webview<R: Runtime>(app: &AppHandle<R>, target: Option<&str>) -> Option<Webview<R>> {
    target
        .and_then(|label| app.get_webview(label))
        .or_else(|| crate::tabs::first_content_webview(app))
}

pub fn handle_id<R: Runtime>(app: &AppHandle<R>, target: Option<&str>, id: &str) {
    match id {
        "quit" => app.exit(0),
        "new-window" => crate::tabs::create_window(app, None),
        "new-tab" => {
            if let Some(webview) = target_webview(app, target) {
                crate::tabs::new_tab_for_webview(app, webview.label());
            }
        }
        "open-stream-window" => {
            if let Some(webview) = target_webview(app, target) {
                crate::tabs::duplicate_in_new_window(app, webview.label());
            } else {
                crate::tabs::create_window(app, None);
            }
        }
        "close-tab" => {
            if let Some(webview) = target_webview(app, target) {
                crate::tabs::close_active_tab(app, webview.label());
            }
        }
        "close-window" => {
            if let Some(webview) = target_webview(app, target) {
                crate::tabs::close_window_for_webview(app, webview.label());
            }
        }
        "toggle-mute" => {
            if let Some(webview) = target_webview(app, target) {
                crate::tabs::toggle_active_mute(app, webview.label());
            }
        }
        "next-tab" => {
            if let Some(webview) = target_webview(app, target) {
                crate::tabs::cycle_for_webview(app, webview.label(), 1);
            }
        }
        "prev-tab" => {
            if let Some(webview) = target_webview(app, target) {
                crate::tabs::cycle_for_webview(app, webview.label(), -1);
            }
        }
        _ if id.starts_with("select-tab-") => {
            if let (Some(webview), Ok(index)) = (
                target_webview(app, target),
                id["select-tab-".len()..].parse::<usize>(),
            ) {
                crate::tabs::select_index_for_webview(app, webview.label(), index);
            }
        }
        "reload" => {
            if let Some(webview) = target_webview(app, target) {
                let _ = webview.reload();
            }
        }
        #[cfg(debug_assertions)]
        "devtools" => {
            if let Some(webview) = target_webview(app, target) {
                webview.open_devtools();
            }
        }
        "fullscreen" => {
            if let Some(webview) = target_webview(app, target) {
                let window = webview.window();
                let fullscreen = window.is_fullscreen().unwrap_or(false);
                let _ = window.set_fullscreen(!fullscreen);
            }
        }
        "restart-server" => {
            if let Some(state) = app.try_state::<std::sync::Arc<ServerState>>() {
                state.restart();
            }
        }
        "open-logs" => {
            if let Some(state) = app.try_state::<std::sync::Arc<ServerState>>() {
                let path = state.log_path().to_string_lossy().to_string();
                let _ = app.opener().open_path(path, None::<&str>);
            }
        }
        "twitch-login" => {
            if let Err(error) = crate::open_twitch_login(app) {
                eprintln!("[twitch-login] failed: {error}");
            }
        }
        "twitch-help" => {
            if let Err(error) = crate::open_twitch_help(app) {
                eprintln!("[twitch-help] failed: {error}");
            }
        }
        "check-updates" => crate::updater::check_manual(app.clone()),
        "about" => {
            if let Err(error) = crate::open_about(app) {
                eprintln!("[about] failed: {error}");
            }
        }
        _ => {}
    }
}

#[tauri::command]
pub fn show_context_menu<R: Runtime>(app: AppHandle<R>, webview: Webview<R>) -> Result<(), String> {
    if let Some(state) = app.try_state::<ContextTarget>() {
        let mut target = state.0.lock().unwrap_or_else(|poison| poison.into_inner());
        *target = Some(webview.label().to_string());
    }
    let menu = build(&app).map_err(|error| error.to_string())?;
    menu.popup(webview.window())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn menu_action<R: Runtime>(app: AppHandle<R>, webview: Webview<R>, id: String) {
    handle_id(&app, Some(webview.label()), &id);
}
