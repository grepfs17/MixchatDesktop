use std::sync::Arc;

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_opener::OpenerExt;

use crate::server::ServerState;

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
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

    let file = Submenu::new(app, "File", true)?;
    file.append(&quit)?;

    let view = Submenu::new(app, "View", true)?;
    view.append(&reload)?;
    #[cfg(debug_assertions)]
    view.append(&devtools)?;
    view.append(&PredefinedMenuItem::separator(app)?)?;
    view.append(&fullscreen)?;

    let account = Submenu::new(app, "Account", true)?;
    account.append(&twitch_login)?;

    let server = Submenu::new(app, "Server", true)?;
    server.append(&restart)?;
    server.append(&logs)?;

    let menu = Menu::new(app)?;
    menu.append(&file)?;
    menu.append(&view)?;
    menu.append(&account)?;
    menu.append(&server)?;
    Ok(menu)
}

pub fn handle<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match event.id().as_ref() {
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
        _ => {}
    }
}
