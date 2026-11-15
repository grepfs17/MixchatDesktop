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

/// Redact the value after sensitive keys in `key=value`, `key: value`,
/// `"key":"value"`, and header forms. Offsets are computed on an ASCII-only
/// lowercase copy, which keeps byte positions identical to the original even
/// when the line contains multi-byte characters.
fn redact_key_values(out: &mut String) {
    const KEYS: &[&str] = &[
        "password",
        "passwd",
        "token",
        "secret",
        "api_key",
        "apikey",
        "authorization",
        "cookie",
        "set-cookie",
        "access_token",
        "refresh_token",
        "id_token",
        "client_secret",
        "code_verifier",
        "potoken",
    ];
    let lower = out.to_ascii_lowercase();
    let lower_bytes = lower.as_bytes();
    let bytes = out.as_bytes();
    let mut edits: Vec<(usize, usize)> = Vec::new();
    for key in KEYS {
        let key_bytes = key.as_bytes();
        let mut from = 0;
        while from + key_bytes.len() <= lower_bytes.len() {
            let Some(rel) = find_bytes(&lower_bytes[from..], key_bytes) else {
                break;
            };
            let key_start = from + rel;
            let mut i = key_start + key_bytes.len();
            if i < lower_bytes.len() && lower_bytes[i] == b'"' {
                i += 1;
            }
            while i < lower_bytes.len() && lower_bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= lower_bytes.len() || (lower_bytes[i] != b'=' && lower_bytes[i] != b':') {
                from = key_start + key_bytes.len();
                continue;
            }
            i += 1;
            while i < lower_bytes.len() && lower_bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= bytes.len() {
                from = key_start + key_bytes.len();
                continue;
            }
            if is_header_key(key) {
                let mut end = i;
                while end < bytes.len() && bytes[end] != b'\r' && bytes[end] != b'\n' {
                    end += 1;
                }
                while end > i && bytes[end - 1].is_ascii_whitespace() {
                    end -= 1;
                }
                if end > i {
                    edits.push((i, end));
                }
                from = end.max(i);
                continue;
            }
            if bytes[i] == b'"' {
                let start = i + 1;
                if let Some(rel_end) = bytes[start..].iter().position(|&b| b == b'"') {
                    let end = start + rel_end;
                    if end > start {
                        edits.push((start, end));
                    }
                    from = end + 1;
                    continue;
                }
            }
            let mut end = i;
            while end < bytes.len() {
                let b = bytes[end];
                if b == b'&' || b == b';' || b == b'"' || b.is_ascii_whitespace() {
                    break;
                }
                end += 1;
            }
            if end > i {
                edits.push((i, end));
            }
            from = end.max(i);
        }
    }
    edits.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in edits {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    for (start, end) in merged.into_iter().rev() {
        if start < end && end <= out.len() {
            out.replace_range(start..end, "***");
        }
    }
}

fn is_header_key(key: &str) -> bool {
    matches!(key, "authorization" | "cookie" | "set-cookie")
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
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

    #[test]
    fn redacts_authorization_header() {
        let out = redact("Authorization: Bearer abc123secret");
        assert!(!out.contains("abc123secret"));
        assert!(out.contains("Authorization: ***"));
    }

    #[test]
    fn redacts_cookie_header() {
        let out = redact("cookie: session=abc123secret; theme=dark");
        assert!(!out.contains("abc123secret"));
        assert!(out.contains("cookie: ***"));
    }

    #[test]
    fn redacts_json_token() {
        let out = redact(r#"{"access_token":"abc123secret","other":1}"#);
        assert!(!out.contains("abc123secret"));
        assert!(out.contains(r#""access_token":"***""#));
    }

    #[test]
    fn redacts_refresh_token_colon() {
        let out = redact("refresh_token: abc123secret");
        assert!(!out.contains("abc123secret"));
        assert!(out.contains("refresh_token: ***"));
    }

    #[test]
    fn redaction_survives_expanding_lowercase() {
        // U+0130 lowercases to two code points, which used to shift offsets
        // computed on the original string.
        let out = redact("İtoken=abc123secret");
        assert!(!out.contains("abc123secret"));
        assert!(out.contains("İtoken=***"));
    }

    #[test]
    fn redacts_multiple_keys_in_one_line() {
        let out = redact("token=abc123secret&secret=def456secret");
        assert!(!out.contains("abc123secret"));
        assert!(!out.contains("def456secret"));
    }
}
