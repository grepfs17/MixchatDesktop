mod config;
mod logger;
mod menu;
mod server;

use std::sync::Arc;

use config::AppConfig;
use logger::Logger;
use server::ServerState;
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .menu(|app| menu::build(app))
        .on_menu_event(|app, event| menu::handle(app, event))
        .setup(|app| {
            app.handle().set_theme(Some(tauri::Theme::Dark));
            let handle = app.handle().clone();
            let app_config = AppConfig::load(&handle)?;
            let logger = Arc::new(Logger::new(&app_config.log_dir)?);

            logger.info(format!(
                "mixchat-desktop starting mode={:?} url={}",
                app_config.mode, app_config.url
            ));

            let state = Arc::new(ServerState::new(
                handle.clone(),
                app_config,
                logger.clone(),
            ));
            app.manage(state.clone());

            #[cfg(unix)]
            {
                let signal_handle = handle.clone();
                let signal_state = state.clone();
                tauri::async_runtime::spawn(async move {
                    use tokio::signal::unix::{signal, SignalKind};
                    let (mut terminate, mut interrupt) = match (
                        signal(SignalKind::terminate()),
                        signal(SignalKind::interrupt()),
                    ) {
                        (Ok(terminate), Ok(interrupt)) => (terminate, interrupt),
                        _ => return,
                    };
                    tokio::select! {
                        _ = terminate.recv() => {}
                        _ = interrupt.recv() => {}
                    }
                    signal_state.shutdown();
                    signal_handle.exit(0);
                });
            }

            tauri::async_runtime::spawn(async move {
                match state.start().await {
                    Ok(()) => {
                        if let Err(error) = create_main_window(&handle, &state) {
                            logger.error(format!("failed to create main window: {error}"));
                            show_error(&handle, &logger, &format!(
                                "Mixchat could not open its window.\n\n{error}"
                            ));
                        }
                    }
                    Err(error) => {
                        logger.error(format!("server failed to start: {error}"));
                        show_error(&handle, &logger, &error);
                        handle.exit(1);
                    }
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the Mixchat desktop app");

    app.run(|app_handle, event| {
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            if let Some(state) = app_handle.try_state::<Arc<ServerState>>() {
                state.shutdown();
            }
        }
    });
}

fn open_twitch_child<R: tauri::Runtime>(
    app: &AppHandle<R>,
    label: &str,
    title: &str,
    url: tauri::Url,
    reload_main_on_close: bool,
) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.navigate(url);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }

    let navigation_app = app.clone();
    let popup_app = app.clone();
    let main_window = app.get_webview_window("main");
    let nav_label = label.to_string();
    let popup_label = label.to_string();
    #[cfg(debug_assertions)]
    eprintln!("[twitch-child] opening {label}");

    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
        .title(title)
        .theme(Some(tauri::Theme::Dark))
        .inner_size(600.0, 760.0)
        .min_inner_size(400.0, 500.0)
        .on_navigation(move |target| {
            let scheme = target.scheme();
            if scheme != "http" && scheme != "https" {
                return false;
            }
            if target.host_str() == Some("www.twitch.tv") && target.path() == "/" {
                if let Some(window) = navigation_app.get_webview_window(&nav_label) {
                    let _ = window.close();
                }
            }
            true
        })
        .on_new_window(move |target, _features| {
            // Keep Twitch login popups (SSO, verification) in the child window
            // so they share the same cookie jar.
            if let Some(window) = popup_app.get_webview_window(&popup_label) {
                let _ = window.navigate(target);
            }
            NewWindowResponse::Deny
        })
        .build()?;
    let _ = window.set_theme(Some(tauri::Theme::Dark));

    // The player only sees the new cookies after its iframe is recreated.
    if reload_main_on_close {
        if let Some(main) = main_window {
            let main = main.clone();
            let _ = window.on_window_event(move |event| {
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    let _ = main.reload();
                }
            });
        }
    }

    Ok(())
}

/// Opens Twitch's own login page in a child webview. The embedded player reads
/// Twitch's session cookies, not the API access token, so this is what makes
/// the player show the signed-in account.
pub(crate) fn open_twitch_login<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> tauri::Result<()> {
    let url: tauri::Url = "https://www.twitch.tv/login".parse().unwrap();
    open_twitch_child(app, "twitch-login", "Log in to Twitch", url, true)
}

/// Opens the Device Code activation page in a child webview. The same login
/// also stores Twitch cookies in the shared WebView2 profile, so the embedded
/// player is signed in without a second login.
pub(crate) fn open_twitch_activation<R: tauri::Runtime>(
    app: &AppHandle<R>,
    url: tauri::Url,
) -> tauri::Result<()> {
    open_twitch_child(app, "twitch-activate", "Authorize Twitch", url, false)
}

fn twitch_activation_target(target: &tauri::Url) -> Option<tauri::Url> {
    if target.path() != "/desktop/twitch-activate" {
        return None;
    }
    let raw = target
        .query_pairs()
        .find(|(key, _)| key.as_ref() == "url")?
        .1
        .to_string();
    let parsed: tauri::Url = raw.parse().ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?;
    if host != "www.twitch.tv" && host != "twitch.tv" {
        return None;
    }
    if parsed.path() != "/activate" {
        return None;
    }
    Some(parsed)
}

fn create_main_window(app: &AppHandle, state: &Arc<ServerState>) -> tauri::Result<()> {
    if app.get_webview_window("main").is_some() {
        return Ok(());
    }

    let mut url: tauri::Url = state
        .url()
        .parse()
        .unwrap_or_else(|_| "http://localhost:3000".parse().unwrap());
    // The query parameter is the reliable shell marker. It survives the
    // webview initialization script being skipped on remote URLs.
    url.query_pairs_mut()
        .append_pair("mixchat-shell", "tauri");
    #[cfg(debug_assertions)]
    eprintln!("[window] loading {url}");

    let navigation_app = app.clone();
    let new_window_app = app.clone();
    let app_host = url.host_str().map(|host| host.to_string());

    let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
        .title("Mixchat")
        .theme(Some(tauri::Theme::Dark))
        .inner_size(1600.0, 900.0)
        .min_inner_size(900.0, 600.0)
        .initialization_script("window.__MIXCHAT_SHELL__ = 'tauri';")
        .on_navigation(move |target| {
            let is_local = matches!(
                target.host_str(),
                Some("localhost") | Some("127.0.0.1") | Some("::1")
            );
            let is_app_host = app_host.as_deref() == target.host_str();
            if is_local || is_app_host {
                if target.path() == "/desktop/twitch-login" {
                    let _ = open_twitch_login(&navigation_app);
                    return false;
                }
                if let Some(activation) = twitch_activation_target(&target) {
                    let _ = open_twitch_activation(&navigation_app, activation);
                    return false;
                }
                return true;
            }
            let _ = navigation_app.opener().open_url(target.as_str(), None::<&str>);
            false
        })
        .on_new_window(move |target, _features| {
            let is_twitch_login = target.path() == "/desktop/twitch-login"
                && matches!(
                    target.host_str(),
                    Some("localhost") | Some("127.0.0.1") | Some("::1")
                );
            if is_twitch_login {
                let _ = open_twitch_login(&new_window_app);
                return NewWindowResponse::Deny;
            }
            let is_local_target = matches!(
                target.host_str(),
                Some("localhost") | Some("127.0.0.1") | Some("::1")
            );
            if is_local_target {
                if let Some(activation) = twitch_activation_target(&target) {
                    let _ = open_twitch_activation(&new_window_app, activation);
                    return NewWindowResponse::Deny;
                }
            }
            let _ = new_window_app
                .opener()
                .open_url(target.as_str(), None::<&str>);
            NewWindowResponse::Deny
        })
        .build()?;
    let _ = window.set_theme(Some(tauri::Theme::Dark));

    let _ = window.set_focus();
    Ok(())
}

fn show_error(app: &AppHandle, logger: &Logger, message: &str) {
    app.dialog()
        .message(message)
        .kind(MessageDialogKind::Error)
        .title("Mixchat failed to start")
        .blocking_show();
    logger.error(format!("logs: {}", logger.path().display()));
}
