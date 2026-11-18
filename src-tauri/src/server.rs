use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::AppHandle;
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

use crate::config::{AppConfig, Mode};
use crate::logger::Logger;
use crate::loopback::LoopbackGuard;

pub struct ServerState {
    app: AppHandle,
    config: AppConfig,
    logger: Arc<Logger>,
    child: Mutex<Option<(u64, CommandChild)>>,
    loopback: Mutex<Option<LoopbackGuard>>,
    shutting_down: AtomicBool,
    restarting: AtomicBool,
    attempts: AtomicU32,
    generation: AtomicU64,
    /// Set once the sidecar prints its "Server listening" line. Proves *our*
    /// child bound the port (an attacker's squatter on :3000 cannot forge
    /// output on the child's stdout pipe).
    ready: AtomicBool,
}

impl ServerState {
    pub fn new(app: AppHandle, config: AppConfig, logger: Arc<Logger>) -> Self {
        Self {
            app,
            config,
            logger,
            child: Mutex::new(None),
            loopback: Mutex::new(None),
            shutting_down: AtomicBool::new(false),
            restarting: AtomicBool::new(false),
            attempts: AtomicU32::new(0),
            generation: AtomicU64::new(0),
            ready: AtomicBool::new(false),
        }
    }

    pub fn url(&self) -> &str {
        &self.config.url
    }

    pub fn mode(&self) -> Mode {
        self.config.mode
    }

    pub fn log_path(&self) -> std::path::PathBuf {
        self.logger.path().to_path_buf()
    }

    pub fn log_info(&self, message: impl AsRef<str>) {
        self.logger.info(message);
    }

    pub fn log_error(&self, message: impl AsRef<str>) {
        self.logger.error(message);
    }

    pub fn restart(self: &Arc<Self>) {
        self.logger.info("restart requested");
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some((_, child)) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
        if self.config.mode == Mode::Dev {
            self.logger.warn("restart ignored in dev mode");
            return;
        }
        self.attempts.store(0, Ordering::SeqCst);
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            // Give the old child time to release the port, then refuse to
            // spawn into an address another process owns.
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let host = state.config.host.clone();
                let port = state.config.port;
                let free =
                    tauri::async_runtime::spawn_blocking(move || !port_in_use(&host, port))
                        .await
                        .unwrap_or(false);
                if free || Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            if let Err(error) = state.spawn() {
                state.logger.error(format!("server restart failed: {error}"));
                state.schedule_restart();
            }
        });
    }

    pub async fn start(self: &Arc<Self>) -> Result<(), String> {
        if self.config.mode == Mode::Dev {
            self.logger.info(format!(
                "dev mode: waiting for external server at {}",
                self.config.url
            ));
            return self.wait_for_health(Duration::from_secs(60)).await;
        }

        if port_in_use(&self.config.host, self.config.port) {
            return Err(format!(
                "Port {} is already in use. Close the other Mixchat instance or set MIXCHAT_PORT.",
                self.config.port
            ));
        }

        self.install_loopback_guard()?;
        self.spawn()?;
        self.wait_for_health(Duration::from_secs(60)).await
    }

    /// Own every loopback address the window hostname resolves to that the
    /// sidecar does not bind, so another local user cannot answer the app
    /// origin through the other address family.
    fn install_loopback_guard(self: &Arc<Self>) -> Result<(), String> {
        if self.loopback.lock().unwrap().is_some() {
            return Ok(());
        }
        let Some((host, url_port)) = self.config.url_host() else {
            return Err(format!("MIXCHAT_URL is not a valid URL: {}", self.config.url));
        };
        if !crate::config::is_loopback_host(&host) {
            return Ok(());
        }
        if url_port != self.config.port {
            return Err(format!(
                "MIXCHAT_URL port {url_port} must match MIXCHAT_PORT {}",
                self.config.port
            ));
        }
        let bind_addr = resolve_addr(&self.config.host, self.config.port)
            .ok_or_else(|| format!("cannot resolve {}:{}", self.config.host, self.config.port))?;
        let guard = LoopbackGuard::install(&host, url_port, bind_addr)?;
        *self.loopback.lock().unwrap() = Some(guard);
        Ok(())
    }

    fn spawn(self: &Arc<Self>) -> Result<(), String> {
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err("app is shutting down".to_string());
        }
        if self.child.lock().unwrap().is_some() {
            return Err("server is already running".to_string());
        }

        if port_in_use(&self.config.host, self.config.port) {
            return Err(format!(
                "port {} is already in use; refusing to start the sidecar",
                self.config.port
            ));
        }

        let entry = self
            .config
            .server_entry
            .as_ref()
            .ok_or_else(|| "server entry is not configured".to_string())?;

        if !entry.is_file() {
            return Err(format!(
                "server build not found at {}. Run pnpm tauri:prepare first.",
                entry.display()
            ));
        }

        let mut command = self
            .app
            .shell()
            .sidecar("mixchat-node")
            .map_err(|error| error.to_string())?
            .arg(entry)
            .current_dir(&self.config.data_dir)
            .set_process_group(true)
            .kill_on_drop(true);

        for (key, value) in &self.config.env {
            command = command.env(key, value);
        }
        // Readiness detection depends on the sidecar's "Server listening"
        // line; never let env (file or inherited) silence it.
        let astro_logging = std::env::var("ASTRO_NODE_LOGGING")
            .ok()
            .or_else(|| self.config.env.get("ASTRO_NODE_LOGGING").cloned());
        if astro_logging
            .map(|v| v.eq_ignore_ascii_case("disabled"))
            .unwrap_or(true)
        {
            command = command.env("ASTRO_NODE_LOGGING", "info");
        }

        let (mut rx, child) = command.spawn().map_err(|error| error.to_string())?;
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.ready.store(false, Ordering::SeqCst);

        self.logger.info(format!(
            "server started pid={} entry={}",
            child.pid(),
            entry.display()
        ));
        *self.child.lock().unwrap() = Some((generation, child));

        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    CommandEvent::Stdout(bytes) => {
                        // The server prints its listening line on startup, so
                        // output means the restart counter can be cleared.
                        state.attempts.store(0, Ordering::SeqCst);
                        let text = String::from_utf8_lossy(&bytes);
                        if text.to_ascii_lowercase().contains("listening") {
                            state.ready.store(true, Ordering::SeqCst);
                        }
                        state.logger.server("stdout", &text);
                    }
                    CommandEvent::Stderr(bytes) => {
                        state
                            .logger
                            .server("stderr", &String::from_utf8_lossy(&bytes));
                    }
                    CommandEvent::Error(error) => {
                        state.logger.error(format!("server process error: {error}"));
                    }
                    CommandEvent::Terminated(payload) => {
                        state.logger.error(format!(
                            "server exited code={:?} signal={:?}",
                            payload.code, payload.signal
                        ));
                        {
                            let mut child = state.child.lock().unwrap();
                            if child.as_ref().map(|(current, _)| *current) == Some(generation) {
                                *child = None;
                            }
                        }
                        if !state.shutting_down.load(Ordering::SeqCst)
                            && state.generation.load(Ordering::SeqCst) == generation
                        {
                            state.schedule_restart();
                        }
                        break;
                    }
                    _ => {}
                }
            }
        });

        Ok(())
    }

    async fn wait_for_health(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let supervised = self.config.mode != Mode::Dev;
        while Instant::now() < deadline {
            if self.shutting_down.load(Ordering::SeqCst) {
                return Err("app is shutting down".to_string());
            }
            // Fail fast if the sidecar died (e.g. lost a port race to a
            // squatter and exited with EADDRINUSE) instead of polling the
            // squatter's HTTP 200 until the timeout.
            if supervised && !self.ready.load(Ordering::SeqCst) {
                let child_alive = self.child.lock().unwrap().is_some();
                if !child_alive {
                    return Err(format!(
                        "server process exited before it became ready on port {} (is another service bound to it?)",
                        self.config.port
                    ));
                }
            }
            let ready = !supervised || self.ready.load(Ordering::SeqCst);
            if ready && http_ok(&self.config.host, self.config.port) {
                // Re-check liveness after the 200: the child must still own
                // the process slot we spawned.
                if supervised && self.child.lock().unwrap().is_none() {
                    return Err("server process exited during health check".to_string());
                }
                self.logger.info(format!(
                    "server ready at http://{}:{}",
                    self.config.host, self.config.port
                ));
                self.attempts.store(0, Ordering::SeqCst);
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(format!(
            "timed out waiting for http://{}:{}",
            self.config.host, self.config.port
        ))
    }

    fn schedule_restart(self: &Arc<Self>) {
        if self.restarting.swap(true, Ordering::SeqCst) {
            return;
        }
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            let attempt = state.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt > 5 {
                state.logger.error("server crashed too many times, giving up");
                state.restarting.store(false, Ordering::SeqCst);
                return;
            }

            let delay_secs = std::cmp::min(30, 1u64 << (attempt - 1));
            state.logger.warn(format!(
                "restarting server in {delay_secs}s (attempt {attempt})"
            ));
            tokio::time::sleep(Duration::from_secs(delay_secs)).await;

            if state.shutting_down.load(Ordering::SeqCst) {
                state.restarting.store(false, Ordering::SeqCst);
                return;
            }

            if let Err(error) = state.spawn() {
                state.logger.error(format!("server restart failed: {error}"));
            }
            state.restarting.store(false, Ordering::SeqCst);
        });
    }

    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        if let Some(guard) = self.loopback.lock().unwrap().take() {
            drop(guard);
        }
        if let Some((_, child)) = self.child.lock().unwrap().take() {
            if let Err(error) = child.kill() {
                self.logger.warn(format!("failed to kill server: {error}"));
            }
        }
    }
}

fn health_host(host: &str) -> &str {
    match host {
        "0.0.0.0" => "127.0.0.1",
        "::" => "::1",
        other => other,
    }
}

fn resolve_addr(host: &str, port: u16) -> Option<SocketAddr> {
    (health_host(host), port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut addresses| addresses.next())
}

fn port_in_use(host: &str, port: u16) -> bool {
    let Some(address) = resolve_addr(host, port) else {
        return false;
    };
    TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
}

fn http_ok(host: &str, port: u16) -> bool {
    let host = health_host(host);
    let Some(address) = resolve_addr(host, port) else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(400)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
    let request = format!(
        "GET /api/parent-domain HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buffer = [0u8; 64];
    match stream.read(&mut buffer) {
        Ok(read) if read > 0 => {
            let status = &buffer[..read];
            status.starts_with(b"HTTP/1.1 200") || status.starts_with(b"HTTP/1.0 200")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn respond_once(status: &'static str) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind responder");
        let port = listener.local_addr().expect("responder address").port();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept probe");
            let mut buffer = [0u8; 512];
            let _ = stream.read(&mut buffer);
            let _ = stream.write_all(status.as_bytes());
            let _ = stream.flush();
        });
        (port, handle)
    }

    fn unused_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind probe");
        let port = listener.local_addr().expect("probe address").port();
        drop(listener);
        port
    }

    #[test]
    fn health_host_maps_wildcard_binds_to_loopback() {
        assert_eq!(health_host("0.0.0.0"), "127.0.0.1");
        assert_eq!(health_host("::"), "::1");
        assert_eq!(health_host("127.0.0.1"), "127.0.0.1");
        assert_eq!(health_host("localhost"), "localhost");
    }

    #[test]
    fn resolve_addr_preserves_port() {
        let address = resolve_addr("127.0.0.1", 3456).expect("resolve loopback");
        assert_eq!(address.port(), 3456);
        assert_eq!(address.ip().to_string(), "127.0.0.1");
    }

    #[test]
    fn port_in_use_detects_listening_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("listener address").port();
        assert!(port_in_use("127.0.0.1", port));
        drop(listener);
        assert!(!port_in_use("127.0.0.1", port));
    }

    #[test]
    fn http_ok_accepts_200_response() {
        let (port, responder) = respond_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        assert!(http_ok("127.0.0.1", port));
        responder.join().expect("join responder");
    }

    #[test]
    fn http_ok_rejects_error_response() {
        let (port, responder) = respond_once("HTTP/1.1 503 Service Unavailable\r\n\r\n");
        assert!(!http_ok("127.0.0.1", port));
        responder.join().expect("join responder");
    }

    #[test]
    fn http_ok_rejects_closed_port() {
        assert!(!http_ok("127.0.0.1", unused_port()));
    }
}
