use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{Manager, Runtime, Url, Webview};
use tauri_plugin_opener::OpenerExt;
use tokio::io::AsyncWriteExt;

use crate::server::ServerState;

/// How often byte counts are pushed into the page while a body is streaming.
const REPORT_INTERVAL: Duration = Duration::from_millis(200);
/// Wait between socket reads before a stalled body is treated as failed.
const READ_TIMEOUT: Duration = Duration::from_secs(45);
/// Finished cards kept in the panel until the user dismisses them.
const KEEP_FINISHED: usize = 8;
/// Error bodies are small JSON objects, never read more than this for a message.
const MAX_ERROR_BODY: usize = 8 * 1024;

pub struct Downloads {
    inner: Arc<Inner>,
}

struct Inner {
    jobs: Mutex<HashMap<String, Job>>,
    next_id: AtomicU64,
    /// Origin of the local Mixchat server. Only downloads from it are handled
    /// here, because only those need the webview cookie jar.
    origin: String,
}

struct Job {
    view: DownloadView,
    cancel: Arc<AtomicBool>,
    created: Instant,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadView {
    id: String,
    filename: String,
    state: DownloadState,
    received: u64,
    total: Option<u64>,
    path: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadState {
    Preparing,
    Running,
    Done,
    Error,
    Cancelled,
}

enum Failure {
    Cancelled,
    Message(String),
}

impl Downloads {
    pub fn new(origin: &str) -> Self {
        let origin = Url::parse(origin)
            .map(|url| url.origin().ascii_serialization())
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                jobs: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(0),
                origin,
            }),
        }
    }
}

/// Installs the ring crypto provider used by the bundled reqwest build. The
/// updater does the same lazily before its first check, so downloads must not
/// depend on an update check having run.
pub fn init_tls() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

/// Starts a download from the page. The injected script cancels same-origin
/// anchor downloads before the webview requests them, so this is the only
/// request the local server sees.
#[tauri::command]
pub fn download_start<R: Runtime>(
    webview: Webview<R>,
    state: tauri::State<'_, Downloads>,
    url: String,
    filename: Option<String>,
) -> Result<(), String> {
    let parsed = Url::parse(&url).map_err(|_| "Invalid download URL.".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Invalid download URL.".to_string());
    }
    if parsed.origin().ascii_serialization() != state.inner.origin {
        return Err("Download URL is not from this app.".to_string());
    }
    let hint = filename
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            parsed
                .path_segments()
                .and_then(|mut segments| segments.next_back())
                .filter(|segment| !segment.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "download".to_string());
    state.inner.start(&webview, parsed, hint);
    Ok(())
}

impl Inner {
    fn start<R: Runtime>(self: &Arc<Self>, webview: &Webview<R>, url: Url, hint: String) {
        let id = format!(
            "download-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed) + 1
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let view = DownloadView {
            id: id.clone(),
            filename: safe_filename(&hint),
            state: DownloadState::Preparing,
            received: 0,
            total: None,
            path: None,
            error: None,
        };

        {
            let mut jobs = lock_jobs(self);
            prune(&mut jobs);
            jobs.insert(
                id.clone(),
                Job {
                    view: view.clone(),
                    cancel: cancel.clone(),
                    created: Instant::now(),
                },
            );
        }
        push(webview, &view);
        log(webview, &format!("download started: {}", view.filename));

        let inner = self.clone();
        let webview = webview.clone();
        tauri::async_runtime::spawn(async move {
            match stream(&inner, &webview, &url, &id, &cancel).await {
                Ok(path) => {
                    log(&webview, &format!("download complete: {}", path.display()));
                    finish(&inner, &webview, &id, DownloadState::Done, Some(path), None);
                }
                Err(Failure::Cancelled) => {
                    log(&webview, "download cancelled");
                    finish(&inner, &webview, &id, DownloadState::Cancelled, None, None);
                }
                Err(Failure::Message(message)) => {
                    log(&webview, &format!("download failed: {message}"));
                    finish(
                        &inner,
                        &webview,
                        &id,
                        DownloadState::Error,
                        None,
                        Some(message),
                    );
                }
            }
        });
    }

    fn views(&self) -> Vec<DownloadView> {
        let jobs = lock_jobs(self);
        let mut views: Vec<&Job> = jobs.values().collect();
        views.sort_by_key(|job| job.created);
        views.into_iter().map(|job| job.view.clone()).collect()
    }

    fn cancel(&self, id: &str) {
        let mut jobs = lock_jobs(self);
        if let Some(job) = jobs.get_mut(id) {
            if matches!(
                job.view.state,
                DownloadState::Preparing | DownloadState::Running
            ) {
                job.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    fn dismiss(&self, id: &str) {
        let mut jobs = lock_jobs(self);
        let running = jobs.get(id).is_some_and(|job| {
            matches!(
                job.view.state,
                DownloadState::Preparing | DownloadState::Running
            )
        });
        if running {
            if let Some(job) = jobs.get_mut(id) {
                job.cancel.store(true, Ordering::Relaxed);
            }
            return;
        }
        jobs.remove(id);
    }

    fn path(&self, id: &str) -> Option<PathBuf> {
        let jobs = lock_jobs(self);
        let job = jobs.get(id)?;
        if job.view.state != DownloadState::Done {
            return None;
        }
        job.view.path.as_ref().map(PathBuf::from)
    }
}

async fn stream<R: Runtime>(
    inner: &Arc<Inner>,
    webview: &Webview<R>,
    url: &Url,
    id: &str,
    cancel: &AtomicBool,
) -> Result<PathBuf, Failure> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(READ_TIMEOUT)
        .build()
        .map_err(|_| Failure::Message("Could not start the download client.".to_string()))?;

    let mut request = client.get(url.clone());
    if let Some(cookies) = cookie_header(webview, url) {
        request = request.header(reqwest::header::COOKIE, cookies);
    }

    let mut response = request
        .send()
        .await
        .map_err(|_| Failure::Message("Could not reach the local Mixchat server.".to_string()))?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = read_error_body(&mut response).await;
        return Err(Failure::Message(status_message(status.as_u16(), &detail)));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Failure::Cancelled);
    }

    let header_name = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .and_then(content_disposition_filename);
    let filename = header_name
        .map(|name| safe_filename(&name))
        .unwrap_or_else(|| current_filename(inner, id));
    let directory = download_dir(webview);
    let total = response.content_length();

    if let Err(error) = tokio::fs::create_dir_all(&directory).await {
        return Err(Failure::Message(format!(
            "Could not open the Downloads folder: {error}"
        )));
    }
    let (path, mut file) = match create_unique_file(&directory, &filename).await {
        Ok(created) => created,
        Err(error) => {
            return Err(Failure::Message(format!(
                "Could not save the file in Downloads: {error}"
            )))
        }
    };

    {
        let mut jobs = lock_jobs(inner);
        if let Some(job) = jobs.get_mut(id) {
            job.view.filename = filename.clone();
            job.view.state = DownloadState::Running;
            job.view.total = total;
        }
    }
    report(inner, webview, id, 0);

    let mut received = 0u64;
    let mut last_report = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return cancel_download(file, &path).await;
        }
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => {
                drop(file);
                remove_file(&path).await;
                return Err(Failure::Message(
                    "The download was interrupted. Check your connection and try again."
                        .to_string(),
                ));
            }
        };
        if let Err(error) = file.write_all(&chunk).await {
            drop(file);
            remove_file(&path).await;
            return Err(Failure::Message(format!(
                "Could not write the downloaded file: {error}"
            )));
        }
        received += chunk.len() as u64;
        if last_report.elapsed() >= REPORT_INTERVAL {
            report(inner, webview, id, received);
            last_report = Instant::now();
        }
    }

    if let Err(error) = file.flush().await {
        drop(file);
        remove_file(&path).await;
        return Err(Failure::Message(format!(
            "Could not finish writing the downloaded file: {error}"
        )));
    }
    drop(file);
    report(inner, webview, id, received);
    Ok(path)
}

async fn cancel_download(file: tokio::fs::File, path: &Path) -> Result<PathBuf, Failure> {
    drop(file);
    remove_file(path).await;
    Err(Failure::Cancelled)
}

async fn remove_file(path: &Path) {
    let _ = tokio::fs::remove_file(path).await;
}

fn cookie_header<R: Runtime>(webview: &Webview<R>, url: &Url) -> Option<String> {
    let cookies = webview.cookies_for_url(url.clone()).ok()?;
    if cookies.is_empty() {
        return None;
    }
    Some(
        cookies
            .iter()
            .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

fn download_dir<R: Runtime>(webview: &Webview<R>) -> PathBuf {
    webview
        .path()
        .download_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
}

fn current_filename(inner: &Inner, id: &str) -> String {
    lock_jobs(inner)
        .get(id)
        .map(|job| job.view.filename.clone())
        .unwrap_or_else(|| "download".to_string())
}

fn report<R: Runtime>(inner: &Inner, webview: &Webview<R>, id: &str, received: u64) {
    let view = {
        let mut jobs = lock_jobs(inner);
        jobs.get_mut(id).map(|job| {
            job.view.received = received;
            job.view.clone()
        })
    };
    if let Some(view) = view {
        push(webview, &view);
    }
}

fn finish<R: Runtime>(
    inner: &Inner,
    webview: &Webview<R>,
    id: &str,
    state: DownloadState,
    path: Option<PathBuf>,
    error: Option<String>,
) {
    let view = {
        let mut jobs = lock_jobs(inner);
        jobs.get_mut(id).map(|job| {
            job.view.state = state;
            job.view.error = error;
            if let Some(path) = path {
                job.view.path = Some(path.to_string_lossy().to_string());
                job.view.total = job.view.total.or(Some(job.view.received));
            }
            job.view.clone()
        })
    };
    if let Some(view) = view {
        push(webview, &view);
    }
}

fn push<R: Runtime>(webview: &Webview<R>, view: &DownloadView) {
    let Ok(json) = serde_json::to_string(view) else {
        return;
    };
    let script =
        format!("window.__MIXCHAT_DOWNLOADS__&&window.__MIXCHAT_DOWNLOADS__.update({json});");
    let _ = webview.eval(script);
}

fn log<R: Runtime>(webview: &Webview<R>, message: &str) {
    if let Some(state) = webview.try_state::<Arc<ServerState>>() {
        state.log_info(format!("[download] {message}"));
    }
}

fn prune(jobs: &mut HashMap<String, Job>) {
    if jobs.len() < KEEP_FINISHED {
        return;
    }
    let mut finished: Vec<(Instant, String)> = jobs
        .values()
        .filter(|job| {
            !matches!(
                job.view.state,
                DownloadState::Preparing | DownloadState::Running
            )
        })
        .map(|job| (job.created, job.view.id.clone()))
        .collect();
    finished.sort_by_key(|(created, _)| *created);
    let excess = jobs.len().saturating_sub(KEEP_FINISHED - 1);
    for (_, id) in finished.into_iter().take(excess) {
        jobs.remove(&id);
    }
}

fn lock_jobs(inner: &Inner) -> std::sync::MutexGuard<'_, HashMap<String, Job>> {
    inner
        .jobs
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

async fn read_error_body(response: &mut reqwest::Response) -> String {
    let mut body = Vec::new();
    while body.len() < MAX_ERROR_BODY {
        match response.chunk().await {
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            Ok(None) => break,
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&body);
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(message) = json.get("error").and_then(|value| value.as_str()) {
            return message.chars().take(300).collect();
        }
    }
    text.trim().chars().take(300).collect()
}

fn status_message(status: u16, detail: &str) -> String {
    match status {
        401 => "Twitch session expired. Log in to Twitch again, then retry.".to_string(),
        429 => "Too many downloads. Try again in a few minutes.".to_string(),
        _ if !detail.is_empty() => format!("Download failed: {detail}"),
        _ => format!("Download failed (HTTP {status})."),
    }
}

/// Pull the filename out of a Content-Disposition header. RFC 5987
/// `filename*=` wins over the plain `filename=` fallback.
fn content_disposition_filename(value: &str) -> Option<String> {
    let params: Vec<&str> = value.split(';').skip(1).map(str::trim).collect();
    for param in &params {
        if let Some(raw) = param.strip_prefix("filename*=") {
            let raw = raw.trim_matches('"');
            if let Some((_, encoded)) = raw.split_once("''") {
                let decoded = percent_decode(encoded);
                if !decoded.is_empty() {
                    return Some(decoded);
                }
            }
        }
    }
    for param in &params {
        if let Some(raw) = param.strip_prefix("filename=") {
            let name = raw.trim().trim_matches('"');
            let unescaped = name.replace("\\\"", "\"").replace("\\\\", "\\");
            if !unescaped.is_empty() {
                return Some(unescaped);
            }
        }
    }
    None
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                out.push(high as u8 * 16 + low as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Keep the filename to a single safe component on every platform.
fn safe_filename(input: &str) -> String {
    let base = input
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(input)
        .trim()
        .trim_matches('.');
    let mut name = String::with_capacity(base.len());
    for ch in base.chars() {
        if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
            name.push('_');
        } else {
            name.push(ch);
        }
    }
    let name: String = name.trim().chars().take(120).collect();
    let name = name.trim().to_string();
    if name.is_empty() {
        return "download".to_string();
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        format!("_{name}")
    } else {
        name
    }
}

/// Create the file without overwriting: WebView2 and WebKit both append " (n)"
/// to existing files, and two downloads of the same name must not collide.
async fn create_unique_file(
    directory: &Path,
    filename: &str,
) -> std::io::Result<(PathBuf, tokio::fs::File)> {
    let mut candidate = directory.join(filename);
    let mut attempt = 0u32;
    loop {
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
            .await
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                attempt += 1;
                if attempt >= 10_000 {
                    return Err(error);
                }
                candidate = numbered_path(directory, filename, attempt);
            }
            Err(error) => return Err(error),
        }
    }
}

fn numbered_path(directory: &Path, filename: &str, counter: u32) -> PathBuf {
    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().to_string());
    let name = match &extension {
        Some(extension) => format!("{stem} ({counter}).{extension}"),
        None => format!("{stem} ({counter})"),
    };
    directory.join(name)
}

#[tauri::command]
pub fn download_snapshot(state: tauri::State<'_, Downloads>) -> Vec<DownloadView> {
    state.inner.views()
}

#[tauri::command]
pub fn download_cancel(state: tauri::State<'_, Downloads>, id: String) {
    state.inner.cancel(&id);
}

#[tauri::command]
pub fn download_dismiss(state: tauri::State<'_, Downloads>, id: String) {
    state.inner.dismiss(&id);
}

#[tauri::command]
pub fn download_reveal(app: tauri::AppHandle, state: tauri::State<'_, Downloads>, id: String) {
    if let Some(path) = state.inner.path(&id) {
        let _ = app.opener().reveal_item_in_dir(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_quoted_content_disposition() {
        assert_eq!(
            content_disposition_filename(r#"attachment; filename="vod-1-chat.json""#),
            Some("vod-1-chat.json".to_string())
        );
        assert_eq!(
            content_disposition_filename("attachment; filename=vod-2.mp4"),
            Some("vod-2.mp4".to_string())
        );
    }

    #[test]
    fn prefers_extended_content_disposition() {
        assert_eq!(
            content_disposition_filename(
                "attachment; filename=\"fallback.json\"; filename*=UTF-8''vod%20chat.json"
            ),
            Some("vod chat.json".to_string())
        );
    }

    #[test]
    fn ignores_missing_content_disposition() {
        assert_eq!(content_disposition_filename("attachment"), None);
    }

    #[test]
    fn strips_paths_and_invalid_characters() {
        assert_eq!(safe_filename(r"C:\Users\me\vod.mp4"), "vod.mp4");
        assert_eq!(safe_filename("../../etc/passwd"), "passwd");
        assert_eq!(safe_filename("a:b*c?.json"), "a_b_c_.json");
        assert_eq!(safe_filename("   "), "download");
        assert_eq!(safe_filename(".hidden"), "hidden");
    }

    #[test]
    fn prefixes_reserved_windows_names() {
        assert_eq!(safe_filename("CON"), "_CON");
        assert_eq!(safe_filename("nul.json"), "_nul.json");
    }

    #[test]
    fn numbers_duplicate_names() {
        let dir = std::env::temp_dir();
        assert_eq!(numbered_path(&dir, "vod.mp4", 1), dir.join("vod (1).mp4"));
        assert_eq!(numbered_path(&dir, "vod", 2), dir.join("vod (2)"));
    }

    #[test]
    fn maps_known_statuses() {
        assert!(status_message(401, "").contains("session expired"));
        assert!(status_message(429, "").contains("Too many downloads"));
        assert_eq!(status_message(502, "boom"), "Download failed: boom");
        assert_eq!(status_message(500, ""), "Download failed (HTTP 500).");
    }
}
