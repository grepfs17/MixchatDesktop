use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::{AppConfig, Mode};
use crate::logger::Logger;
use crate::loopback::LoopbackGuard;

pub struct ServerState {
    config: AppConfig,
    logger: Arc<Logger>,
    child: Mutex<Option<(u64, Sidecar)>>,
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

/// A running sidecar. The monitor thread polls it, so `kill` performs the
/// platform tree kill and reaps the process here, before the app moves on.
struct Sidecar {
    child: Arc<Mutex<Option<Child>>>,
}

impl Sidecar {
    fn kill(&self, logger: &Logger) {
        let mut guard = self.child.lock().unwrap();
        let Some(child) = guard.as_mut() else {
            return;
        };
        if let Err(error) = kill_tree(child) {
            logger.warn(format!("failed to kill server: {error}"));
            return;
        }
        let _ = child.wait();
        *guard = None;
    }
}

impl Drop for ServerState {
    fn drop(&mut self) {
        if let Ok(slot) = self.child.get_mut() {
            if let Some((_, sidecar)) = slot.take() {
                sidecar.kill(&self.logger);
            }
        }
    }
}

impl ServerState {
    pub fn new(config: AppConfig, logger: Arc<Logger>) -> Self {
        Self {
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
        if let Some((_, sidecar)) = self.child.lock().unwrap().take() {
            sidecar.kill(&self.logger);
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

        let binary = sidecar_path()?;
        let mut command = Command::new(&binary);
        command
            .arg(entry)
            .current_dir(&self.config.data_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, value) in &self.config.env {
            command.env(key, value);
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
            command.env("ASTRO_NODE_LOGGING", "info");
        }

        // Its own process group (POSIX) and the hidden window flag (Windows)
        // let a kill reach the whole tree and keep the console hidden.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut process = command.spawn().map_err(|error| {
            format!("failed to start {}: {error}", binary.display())
        })?;
        let pid = process.id();
        let stdout = process.stdout.take();
        let stderr = process.stderr.take();
        let process = Arc::new(Mutex::new(Some(process)));

        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.ready.store(false, Ordering::SeqCst);
        *self.child.lock().unwrap() = Some((
            generation,
            Sidecar {
                child: process.clone(),
            },
        ));

        self.logger.info(format!(
            "server started pid={pid} entry={}",
            entry.display()
        ));

        let mut readers = Vec::new();
        if let Some(pipe) = stdout {
            let state = self.clone();
            readers.push(std::thread::spawn(move || {
                read_server_output(&state, pipe, "stdout")
            }));
        }
        if let Some(pipe) = stderr {
            let state = self.clone();
            readers.push(std::thread::spawn(move || {
                read_server_output(&state, pipe, "stderr")
            }));
        }

        let state = self.clone();
        std::thread::spawn(move || {
            let exit = loop {
                let mut guard = process.lock().unwrap();
                let Some(child) = guard.as_mut() else {
                    // An explicit kill reaped it and already took the status.
                    break None;
                };
                let outcome = child.try_wait();
                drop(guard);
                match outcome {
                    Ok(Some(status)) => break Some(Ok(status)),
                    Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                    Err(error) => break Some(Err(error)),
                }
            };

            // Drain the pipes before reporting, so the last lines land in
            // order ahead of the exit notice.
            for reader in readers {
                let _ = reader.join();
            }

            match exit {
                Some(Ok(status)) => state.logger.error(format!(
                    "server exited code={:?} signal={:?}",
                    status.code(),
                    exit_signal(&status)
                )),
                Some(Err(error)) => {
                    state.logger.error(format!("server process error: {error}"));
                }
                None => {}
            }

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
        if let Some((_, sidecar)) = self.child.lock().unwrap().take() {
            sidecar.kill(&self.logger);
        }
    }
}

/// Resolve the bundled sidecar next to the running executable, matching how
/// Tauri places `externalBin` files.
fn sidecar_path() -> Result<PathBuf, String> {
    let executable = tauri::utils::platform::current_exe()
        .map_err(|error| format!("cannot locate the app executable: {error}"))?;
    let directory = executable
        .parent()
        .ok_or_else(|| "app executable has no parent directory".to_string())?;
    // Tests run from target/<profile>/deps, one level below the binaries.
    let directory = if directory.ends_with("deps") {
        directory.parent().unwrap_or(directory)
    } else {
        directory
    };
    let path = directory.join("mixchat-node");
    #[cfg(windows)]
    let path = {
        let mut path = path;
        path.as_mut_os_string().push(".exe");
        path
    };
    Ok(path)
}

/// Read one sidecar pipe line by line. Line breaks are kept so the log file
/// records exactly what the server printed.
fn read_server_output(state: &Arc<ServerState>, pipe: impl Read, stream: &'static str) {
    let mut reader = BufReader::new(pipe);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buffer);
                if stream == "stdout" {
                    // The server prints its listening line on startup, so
                    // output means the restart counter can be cleared.
                    state.attempts.store(0, Ordering::SeqCst);
                    if text.to_ascii_lowercase().contains("listening") {
                        state.ready.store(true, Ordering::SeqCst);
                    }
                }
                state.logger.server(stream, &text);
            }
            Err(error) => {
                state
                    .logger
                    .error(format!("server {stream} read error: {error}"));
                break;
            }
        }
    }
}

#[cfg(unix)]
fn kill_tree(child: &mut Child) -> std::io::Result<()> {
    // The sidecar runs as its own process group, so a signal reaches helpers
    // it started as well.
    let pgid = child.id() as libc::pid_t;
    if unsafe { libc::killpg(pgid, libc::SIGKILL) } != 0 {
        let error = std::io::Error::last_os_error();
        // ESRCH: every process in the group already exited.
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
fn kill_tree(child: &mut Child) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;

    // taskkill removes the tree, which is what the job object used to do.
    // Fall back to the direct process if it fails.
    let pid = child.id().to_string();
    let killed = Command::new("taskkill")
        .args(["/PID", pid.as_str(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !killed {
        child.kill()?;
    }
    Ok(())
}

#[cfg(unix)]
fn exit_signal(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn exit_signal(_status: &std::process::ExitStatus) -> Option<i32> {
    None
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
