use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

pub struct Logger {
    file: Mutex<Option<File>>,
    path: PathBuf,
}

impl Logger {
    pub fn new(log_dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(log_dir)?;
        let path = log_dir.join("mixchat-server.log");

        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > MAX_LOG_BYTES {
                let _ = std::fs::rename(&path, log_dir.join("mixchat-server.log.1"));
            }
        }

        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            file: Mutex::new(Some(file)),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn info(&self, message: impl AsRef<str>) {
        self.write("INFO", message.as_ref());
    }

    pub fn warn(&self, message: impl AsRef<str>) {
        self.write("WARN", message.as_ref());
    }

    pub fn error(&self, message: impl AsRef<str>) {
        self.write("ERROR", message.as_ref());
    }

    pub fn server(&self, stream: &str, message: &str) {
        self.write("SERVER", &format!("[{stream}] {}", redact(message)));
    }

    fn write(&self, level: &str, message: &str) {
        let line = format!(
            "[{}] [{level}] {message}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        eprintln!("{line}");
        if let Ok(mut guard) = self.file.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = writeln!(file, "{line}");
                let _ = file.flush();
            }
        }
    }
}

/// Strip credentials from URLs and obvious secrets before a line reaches the
/// log file (sidecar output is written verbatim and persists across restarts).
fn redact(input: &str) -> String {
    let mut out = redact_url_credentials(input);
    redact_key_values(&mut out);
    out
}

/// `scheme://user:pass@host` → `scheme://user:***@host`
fn redact_url_credentials(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find("://") {
        let (before, from_scheme) = rest.split_at(pos + 3);
        result.push_str(before);
        // Authority ends at first path/query/fragment/space delimiter.
        let authority_len = from_scheme
            .find(|c: char| c == '/' || c == '?' || c == '#' || c.is_whitespace())
            .unwrap_or(from_scheme.len());
        let (authority, after_authority) = from_scheme.split_at(authority_len);
        if let Some(at) = authority.find('@') {
            let userinfo = &authority[..at];
            if let Some(colon) = userinfo.find(':') {
                result.push_str(&userinfo[..colon]);
                result.push_str(":***@");
                result.push_str(&authority[at + 1..]);
            } else {
                result.push_str(authority);
            }
        } else {
            result.push_str(authority);
        }
        rest = after_authority;
    }
    result.push_str(rest);
    result
}

/// Redact the value after sensitive `key=` / `key:` markers.
fn redact_key_values(out: &mut String) {
    const MARKERS: &[&str] = &[
        "password=",
        "passwd=",
        "token=",
        "secret=",
        "api_key=",
        "apikey=",
        "authorization:",
        "cookie:",
    ];
    let lower = out.to_lowercase();
    // Collect edits first so replacements don't shift later offsets.
    let mut edits: Vec<(usize, usize)> = Vec::new();
    for marker in MARKERS {
        let mut search_from = 0;
        while let Some(rel) = lower[search_from..].find(marker) {
            let value_start = search_from + rel + marker.len();
            let value_end = out[value_start..]
                .find(|c: char| c == '&' || c == ';' || c.is_whitespace() || c == '"')
                .map(|i| value_start + i)
                .unwrap_or(out.len());
            if value_end > value_start {
                edits.push((value_start, value_end));
            }
            search_from = value_start;
        }
    }
    edits.sort();
    for (start, end) in edits.into_iter().rev() {
        out.replace_range(start..end, "***");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_url_password() {
        let out = redact("fetch http://user:secretpw@example.com/api failed");
        assert!(!out.contains("secretpw"));
        assert!(out.contains("user:***@example.com"));
    }

    #[test]
    fn redacts_token_kv() {
        let out = redact("auth failed token=abc123secret here");
        assert!(!out.contains("abc123secret"));
        assert!(out.contains("token=***"));
    }

    #[test]
    fn leaves_plain_text() {
        assert_eq!(
            redact("Server listening on http://127.0.0.1:3000"),
            "Server listening on http://127.0.0.1:3000"
        );
    }

    #[test]
    fn redacts_multiple_urls() {
        let out = redact("a http://u:p@one.test/x b https://ok.test/y c");
        assert!(!out.contains(":p@"));
        assert!(out.contains("u:***@one.test"));
        assert!(out.contains("https://ok.test/y"));
    }
}
