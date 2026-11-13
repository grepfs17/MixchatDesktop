use std::sync::Arc;

use tauri::menu::{
    ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem,
};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_opener::OpenerExt;

use crate::server::ServerState;

/// Flat menu shown on right-click. Submenus are inlined because popup menus
/// render one level at a time.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let copy = PredefinedMenuItem::copy(app, Some("Copy"))?;
    let cut = PredefinedMenuItem::cut(app, Some("Cut"))?;
    let paste = PredefinedMenuItem::paste(app, Some("Paste"))?;
    let select_all = PredefinedMenuItem::select_all(app, Some("Select All"))?;

    let version = app.package_info().version.to_string();
    let about =
        MenuItem::with_id(app, "about", format!("About Mixchat ({version})"), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, Some("CmdOrCtrl+Q"))?;
    let reload = MenuItem::with_id(app, "reload", "Reload", true, Some("CmdOrCtrl+R"))?;
    #[cfg(debug_assertions)]
    let devtools = MenuItem::with_id(app, "devtools", "Toggle DevTools", true, Some("F12"))?;
    let fullscreen =
        MenuItem::with_id(app, "fullscreen", "Toggle Fullscreen", true, Some("F11"))?;
    let restart = MenuItem::with_id(app, "restart-server", "Restart server", true, None::<&str>)?;
    let logs = MenuItem::with_id(app, "open-logs", "Open logs", true, None::<&str>)?;
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
    menu.append(&about)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&quit)?;
    Ok(menu)
}

pub fn handle<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    handle_id(app, event.id().as_ref());
}

pub fn handle_id<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match id {
        "quit" => app.exit(0),
        "reload" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.eval("window.location.reload()");
            }
        }
        #[cfg(debug_assertions)]
        "devtools" => {
            if let Some(window) = app.get_webview_window("main") {
                window.open_devtools();
            }
        }
        "fullscreen" => {
            if let Some(window) = app.get_webview_window("main") {
                let fullscreen = window.is_fullscreen().unwrap_or(false);
                let _ = window.set_fullscreen(!fullscreen);
            }
        }
        "restart-server" => {
            if let Some(state) = app.try_state::<Arc<ServerState>>() {
                state.restart();
            }
        }
        "open-logs" => {
            if let Some(state) = app.try_state::<Arc<ServerState>>() {
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
        "about" => {
            if let Err(error) = crate::open_about(app) {
                eprintln!("[about] failed: {error}");
            }
        }
        _ => {}
    }
}

#[tauri::command]
pub fn show_context_menu(app: AppHandle) -> Result<(), String> {
    let webview = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    let menu = build(&app).map_err(|error| error.to_string())?;
    menu.popup(webview.as_ref().window())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn menu_action(app: AppHandle, id: String) {
    handle_id(&app, &id);
}
