use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::utils::config::BundleType;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::config::Mode;
use crate::server::ServerState;

static CHECK_RUNNING: AtomicBool = AtomicBool::new(false);

const PORTABLE_TARGET: &str = "windows-x86_64-portable";

pub fn is_supported_install() -> bool {
    match tauri::utils::platform::bundle_type() {
        Some(BundleType::Nsis) => cfg!(windows),
        Some(BundleType::AppImage) => cfg!(target_os = "linux"),
        Some(BundleType::App) => cfg!(target_os = "macos"),
        _ => false,
    }
}

/// The portable zip is an unbundled build, so Tauri reports no bundle type and
/// the executable has been renamed to `Mixchat.exe` by the packaging script.
pub fn is_portable_install() -> bool {
    #[cfg(windows)]
    {
        tauri::utils::platform::bundle_type().is_none()
            && std::env::current_exe()
                .ok()
                .and_then(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().eq_ignore_ascii_case("Mixchat.exe"))
                })
                .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub fn updates_supported() -> bool {
    is_supported_install() || is_portable_install()
}

pub fn check_manual<R: Runtime>(app: AppHandle<R>) {
    start(app, true);
}

/// Check quietly after launch. Only called for packaged builds.
pub fn check_in_background<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        start(app, false);
    });
}

fn start<R: Runtime>(app: AppHandle<R>, interactive: bool) {
    if CHECK_RUNNING.swap(true, Ordering::SeqCst) {
        if interactive {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                show_message(&app, "Mixchat", "An update check is already running.").await;
            });
        }
        return;
    }

    tauri::async_runtime::spawn(async move {
        let result = check(&app, interactive).await;
        CHECK_RUNNING.store(false, Ordering::SeqCst);
        if let Err(error) = result {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title("Mixchat");
            }
            log_error(&app, format!("update check failed: {error}"));
            if interactive {
                show_message(
                    &app,
                    "Update failed",
                    &format!("Mixchat could not update.\n\n{error}"),
                )
                .await;
            }
        }
    });
}

async fn check<R: Runtime>(app: &AppHandle<R>, interactive: bool) -> Result<(), String> {
    let packaged = app
        .try_state::<Arc<ServerState>>()
        .map(|state| state.mode() == Mode::Packaged)
        .unwrap_or(false);
    let portable = is_portable_install();
    if !packaged || (!is_supported_install() && !portable) {
        if interactive {
            show_message(
                app,
                "Updates unavailable",
                "Automatic updates are only available in the installed Windows build, the Linux AppImage, and the portable Windows zip.",
            )
            .await;
        }
        return Ok(());
    }

    let updater = if portable {
        app.updater_builder().target(PORTABLE_TARGET).build()
    } else {
        app.updater()
    }
    .map_err(|error| error.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|error| error.to_string())?;

    let Some(update) = update else {
        if interactive {
            let version = app.package_info().version.to_string();
            show_message(
                app,
                "No updates",
                &format!("Mixchat {version} is up to date."),
            )
            .await;
        }
        return Ok(());
    };

    log_info(
        app,
        format!(
            "update available: {} (current {})",
            update.version, update.current_version
        ),
    );

    let mut message = format!(
        "Mixchat {} is available. You are running {}.\n\n",
        update.version, update.current_version
    );
    if let Some(notes) = update
        .body
        .as_deref()
        .map(str::trim)
        .filter(|notes| !notes.is_empty())
    {
        message.push_str(&truncate(notes, 1200));
        message.push_str("\n\n");
    }
    message.push_str("Download and install now?");

    if !ask(
        app,
        "Update available",
        &message,
        "Download and install",
        "Later",
    )
    .await
    {
        log_info(app, "update postponed");
        return Ok(());
    }

    log_info(app, format!("downloading update {}", update.version));
    if portable {
        install_portable(app, &update).await?;
        return Ok(());
    }

    let window = app.get_webview_window("main");
    let mut downloaded = 0u64;
    let mut last_percent = 0u8;

    update
        .download_and_install(
            |chunk_length, content_length| {
                downloaded += chunk_length as u64;
                let Some(total) = content_length.filter(|total| *total > 0) else {
                    return;
                };
                let percent = downloaded
                    .saturating_mul(100)
                    .saturating_div(total)
                    .min(100) as u8;
                if percent < last_percent.saturating_add(1) && percent != 100 {
                    return;
                }
                last_percent = percent;
                if let Some(window) = &window {
                    let _ = window.set_title(&format!("Mixchat - Downloading update {percent}%"));
                }
                log_info(app, format!("update download {percent}%"));
            },
            || {
                if let Some(window) = &window {
                    let _ = window.set_title("Mixchat - Installing update");
                }
                log_info(app, "update download complete");
            },
        )
        .await
        .map_err(|error| error.to_string())?;

    if let Some(window) = &window {
        let _ = window.set_title("Mixchat");
    }
    log_info(app, format!("update {} installed", update.version));

    #[cfg(not(windows))]
    {
        if ask(
            app,
            "Update installed",
            "Restart Mixchat now to use the new version?",
            "Restart now",
            "Later",
        )
        .await
        {
            app.restart();
        }
    }

    Ok(())
}

#[cfg(windows)]
const PORTABLE_UPDATER_SCRIPT: &str = r##"param(
    [Parameter(Mandatory = $true)][int]$WaitPid,
    [Parameter(Mandatory = $true)][string]$Zip,
    [Parameter(Mandatory = $true)][string]$Target,
    [Parameter(Mandatory = $true)][string]$Exe
)

$ErrorActionPreference = "Stop"
$LogDir = Join-Path $Target "app-data\logs"
$Log = Join-Path $LogDir "updater.log"
$Stage = Join-Path ([System.IO.Path]::GetTempPath()) ("mixchat-portable-" + [Guid]::NewGuid().ToString("N"))
$ScriptPath = $PSCommandPath

function Write-Log([string]$Message) {
    try {
        New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
        Add-Content -LiteralPath $Log -Value ("[{0}] {1}" -f (Get-Date -Format o), $Message)
    } catch {
    }
}

function Copy-Staged([string]$Source, [string]$Destination, [string]$ExeName) {
    $items = Get-ChildItem -LiteralPath $Source -Force |
        Where-Object { $_.Name -ne "app-data" } |
        Sort-Object { if ($_.Name -eq $ExeName) { 1 } else { 0 } }

    foreach ($item in $items) {
        if ($item.Name -eq $ExeName) {
            $pending = Join-Path $Destination ("." + $ExeName + ".new")
            $destinationExe = Join-Path $Destination $ExeName
            Copy-Item -LiteralPath $item.FullName -Destination $pending -Force
            if (Test-Path -LiteralPath $destinationExe) {
                [System.IO.File]::Replace($pending, $destinationExe, $null)
            } else {
                Move-Item -LiteralPath $pending -Destination $destinationExe -Force
            }
        } else {
            Copy-Item -LiteralPath $item.FullName -Destination $Destination -Recurse -Force
        }
    }
}

try {
    Write-Log "waiting for Mixchat (pid $WaitPid) to exit"
    $deadline = (Get-Date).AddSeconds(120)
    while ((Get-Process -Id $WaitPid -ErrorAction SilentlyContinue) -and ((Get-Date) -lt $deadline)) {
        Start-Sleep -Milliseconds 250
    }

    Start-Sleep -Seconds 2
    Write-Log "extracting $Zip"
    New-Item -ItemType Directory -Force -Path $Stage | Out-Null
    Expand-Archive -LiteralPath $Zip -DestinationPath $Stage -Force

    $source = Join-Path $Stage "Mixchat"
    if (-not (Test-Path -LiteralPath $source)) {
        $source = $Stage
    }

    $copied = $false
    for ($attempt = 1; $attempt -le 10; $attempt++) {
        try {
            Copy-Staged -Source $source -Destination $Target -ExeName $Exe
            $copied = $true
            break
        } catch {
            Write-Log ("copy attempt {0} failed: {1}" -f $attempt, $_.Exception.Message)
            Start-Sleep -Seconds 1
        }
    }

    if (-not $copied) {
        throw "could not replace the portable files"
    }
    Write-Log "update applied"
} catch {
    Write-Log ("update failed: {0}" -f $_.Exception.Message)
} finally {
    try {
        if (Test-Path -LiteralPath $Stage) {
            Remove-Item -LiteralPath $Stage -Recurse -Force
        }
        if (Test-Path -LiteralPath $Zip) {
            Remove-Item -LiteralPath $Zip -Force
        }
        if (Test-Path -LiteralPath $ScriptPath) {
            Remove-Item -LiteralPath $ScriptPath -Force
        }
    } catch {
    }

    $app = Join-Path $Target $Exe
    if (Test-Path -LiteralPath $app) {
        Write-Log "starting $app"
        Start-Process -FilePath $app -WorkingDirectory $Target
    } else {
        Write-Log "application executable not found"
    }
}
"##;

async fn install_portable<R: Runtime>(app: &AppHandle<R>, update: &Update) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = (app, update);
        return Err("portable updates are only supported on Windows".to_string());
    }

    #[cfg(windows)]
    {
        use std::io::Write;
        use std::os::windows::process::CommandExt;

        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let target = exe
            .parent()
            .ok_or_else(|| "cannot find the portable application directory".to_string())?;
        let exe_name = exe
            .file_name()
            .ok_or_else(|| "cannot find the portable executable name".to_string())?
            .to_string_lossy()
            .to_string();

        let window = app.get_webview_window("main");
        let mut downloaded = 0u64;
        let mut last_percent = 0u8;
        let bytes = update
            .download(
                |chunk_length, content_length| {
                    downloaded += chunk_length as u64;
                    let Some(total) = content_length.filter(|total| *total > 0) else {
                        return;
                    };
                    let percent = downloaded
                        .saturating_mul(100)
                        .saturating_div(total)
                        .min(100) as u8;
                    if percent < last_percent.saturating_add(1) && percent != 100 {
                        return;
                    }
                    last_percent = percent;
                    if let Some(window) = &window {
                        let _ =
                            window.set_title(&format!("Mixchat - Downloading update {percent}%"));
                    }
                    log_info(app, format!("update download {percent}%"));
                },
                || {},
            )
            .await
            .map_err(|error| error.to_string())?;

        if let Some(window) = &window {
            let _ = window.set_title("Mixchat - Installing update");
        }

        let mut zip = tempfile::Builder::new()
            .prefix("mixchat-portable-update-")
            .suffix(".zip")
            .tempfile()
            .map_err(|error| error.to_string())?;
        zip.write_all(&bytes)
            .map_err(|error| error.to_string())?;
        zip.flush().map_err(|error| error.to_string())?;
        let (zip_file, zip_path) = zip.keep().map_err(|error| error.to_string())?;
        drop(zip_file);

        let mut script = tempfile::Builder::new()
            .prefix("mixchat-portable-updater-")
            .suffix(".ps1")
            .tempfile()
            .map_err(|error| error.to_string())?;
        script
            .write_all(PORTABLE_UPDATER_SCRIPT.as_bytes())
            .map_err(|error| error.to_string())?;
        script.flush().map_err(|error| error.to_string())?;
        let (script_file, script_path) = script.keep().map_err(|error| error.to_string())?;
        drop(script_file);

        show_message(
            app,
            "Update ready",
            "Mixchat will restart to finish installing the update.",
        )
        .await;

        let mut command = std::process::Command::new("powershell.exe");
        command
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-WindowStyle")
            .arg("Hidden")
            .arg("-File")
            .arg(&script_path)
            .arg("-WaitPid")
            .arg(std::process::id().to_string())
            .arg("-Zip")
            .arg(&zip_path)
            .arg("-Target")
            .arg(target)
            .arg("-Exe")
            .arg(&exe_name);
        command.creation_flags(0x08000000);

        if let Err(error) = command.spawn() {
            let _ = std::fs::remove_file(&script_path);
            let _ = std::fs::remove_file(&zip_path);
            return Err(format!(
                "failed to start the portable updater: {error}"
            ));
        }

        log_info(
            app,
            format!("portable update {} staged for install", update.version),
        );
        app.exit(0);
        Ok(())
    }
}

async fn ask<R: Runtime>(
    app: &AppHandle<R>,
    title: &str,
    message: &str,
    ok: &str,
    cancel: &str,
) -> bool {
    let app = app.clone();
    let title = title.to_string();
    let message = message.to_string();
    let ok = ok.to_string();
    let cancel = cancel.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .message(message)
            .title(title)
            .kind(MessageDialogKind::Info)
            .buttons(MessageDialogButtons::OkCancelCustom(ok, cancel))
            .blocking_show()
    })
    .await
    .unwrap_or(false)
}

async fn show_message<R: Runtime>(app: &AppHandle<R>, title: &str, message: &str) {
    let app = app.clone();
    let title = title.to_string();
    let message = message.to_string();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .message(message)
            .title(title)
            .kind(MessageDialogKind::Info)
            .buttons(MessageDialogButtons::Ok)
            .blocking_show()
    })
    .await;
}

fn log_info<R: Runtime>(app: &AppHandle<R>, message: impl AsRef<str>) {
    if let Some(state) = app.try_state::<Arc<ServerState>>() {
        state.log_info(message);
    } else {
        eprintln!("[update] {}", message.as_ref());
    }
}

fn log_error<R: Runtime>(app: &AppHandle<R>, message: impl AsRef<str>) {
    if let Some(state) = app.try_state::<Arc<ServerState>>() {
        state.log_error(message);
    } else {
        eprintln!("[update] {}", message.as_ref());
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncate_keeps_text_at_or_under_limit() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn truncate_adds_ellipsis_only_when_cut() {
        assert_eq!(truncate("hello!", 5), "hello...");
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        assert_eq!(truncate("áéíóú", 3), "áéí...");
    }
}
