use anyhow::{Context, Result};
use ssh2::{CheckResult, KeyboardInteractivePrompt, KnownHostFileKind, KnownHostKeyFormat, Prompt, Session};
use std::io::Read;
use std::net::TcpStream;
use std::time::Duration;
use zeroize::Zeroizing;

use crate::config::Location;


/// Apply OS-level TCP keepalives to `tcp` so the kernel automatically sends
/// probe packets during idle periods (e.g. while a remote `find` traverses
/// slow/network-mounted symlinked directories).  Without this, a NAT or
/// firewall silently drops the "idle" TCP connection after ~60 s and the
/// next read returns ETIMEDOUT.  OS-level keepalives require no application
/// polling — the kernel handles them entirely.
fn set_tcp_keepalive(tcp: &TcpStream) {
    use socket2::{SockRef, TcpKeepalive};
    // SockRef borrows the fd without taking ownership — no double-close.
    let sock = SockRef::from(tcp);
    let ka = TcpKeepalive::new()
        .with_time(Duration::from_secs(30))     // idle before first probe
        .with_interval(Duration::from_secs(10)) // interval between probes
        .with_retries(6);                        // fail after 6 missed probes
    let _ = sock.set_tcp_keepalive(&ka);
}

pub struct SshClient {
    pub session: Session,
}

struct PasswordHandler {
    password: Zeroizing<String>,
}

impl KeyboardInteractivePrompt for PasswordHandler {
    fn prompt(&mut self, _username: &str, _instructions: &str, prompts: &[Prompt<'_>]) -> Vec<String> {
        // Respond with the password to every prompt (covers password, OTP, etc.)
        vec![self.password.as_str().to_string(); prompts.len()]
    }
}

/// Verify the server's host key against the known_hosts file (TOFU).
/// First connection: key is stored and trusted automatically.
/// Subsequent connections: key must match the stored entry.
/// Key mismatch: connection refused with a warning.
fn check_host_key(session: &Session, host: &str, port: u16) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let (key, key_type) = session
        .host_key()
        .ok_or_else(|| anyhow::anyhow!("Server provided no host key"))?;

    let fmt = KnownHostKeyFormat::from(key_type);
    if matches!(fmt, KnownHostKeyFormat::Unknown) {
        return Err(anyhow::anyhow!("Server presented an unknown host key type"));
    }

    let known_hosts_path = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from(".config"))
        .join("booker-search")
        .join("known_hosts");

    let mut known_hosts = session.known_hosts().context("Failed to initialise known_hosts")?;
    if known_hosts_path.exists() {
        known_hosts
            .read_file(&known_hosts_path, KnownHostFileKind::OpenSSH)
            .context("Failed to read known_hosts file")?;
    }

    // For non-standard ports OpenSSH uses [host]:port notation in known_hosts.
    let host_entry = if port == 22 {
        host.to_string()
    } else {
        format!("[{}]:{}", host, port)
    };

    match known_hosts.check_port(host, port, key) {
        CheckResult::Match => Ok(()),
        CheckResult::NotFound => {
            // Trust-on-first-use: store the key and proceed.
            known_hosts
                .add(&host_entry, key, "booker-search", fmt)
                .context("Failed to add host key to known_hosts")?;
            if let Some(parent) = known_hosts_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            known_hosts
                .write_file(&known_hosts_path, KnownHostFileKind::OpenSSH)
                .context("Failed to write known_hosts file")?;
            std::fs::set_permissions(
                &known_hosts_path,
                std::fs::Permissions::from_mode(0o600),
            )?;
            Ok(())
        }
        CheckResult::Mismatch => Err(anyhow::anyhow!(
            "Host key mismatch for {} — the server's key has changed. \
             This may indicate a man-in-the-middle attack. \
             Remove the stale entry from {:?} to reconnect.",
            host_entry,
            known_hosts_path
        )),
        CheckResult::Failure => Err(anyhow::anyhow!(
            "Failed to check host key for {}",
            host_entry
        )),
    }
}

impl SshClient {
    pub fn connect(location: &Location, password: &str) -> Result<Self> {
        let host = location.ssh_host.as_deref().unwrap_or("localhost");
        let port = location.ssh_port;
        let user = location.ssh_user.as_deref().unwrap_or("root");

        let addr = format!("{}:{}", host, port);
        let tcp = TcpStream::connect_timeout(
            &addr.parse().with_context(|| format!("Invalid address: {}", addr))?,
            Duration::from_secs(10),
        )
        .with_context(|| format!("Failed to connect to {}", addr))?;

        // OS-level TCP keepalives keep the connection alive during long find
        // traversals that produce no output for extended periods.
        set_tcp_keepalive(&tcp);

        let mut session = Session::new().context("Failed to create SSH session")?;
        session.set_tcp_stream(tcp);
        session.set_timeout(30000); // 30s timeout for handshake; set to 0 after auth
        session.handshake().with_context(|| format!("SSH handshake failed with {}", addr))?;
        session.set_timeout(0); // no per-operation timeout — long find commands need this

        // Verify host key (TOFU — trust on first use, refuse on mismatch).
        check_host_key(&session, host, port)?;

        // Try SSH agent first (uses existing key auth if available)
        if let Ok(mut agent) = session.agent() {
            if agent.connect().is_ok() && agent.list_identities().is_ok() {
                for identity in agent.identities().unwrap_or_default() {
                    if agent.userauth(user, &identity).is_ok() && session.authenticated() {
                        return Ok(Self { session });
                    }
                }
            }
        }

        // Try password auth
        let _ = session.userauth_password(user, password);
        if session.authenticated() {
            return Ok(Self { session });
        }

        // Try keyboard-interactive (some servers require this even with PasswordAuthentication yes)
        let _ = session.userauth_keyboard_interactive(user, &mut PasswordHandler {
            password: Zeroizing::new(password.to_string()),
        });
        if session.authenticated() {
            return Ok(Self { session });
        }

        Err(anyhow::anyhow!("SSH authentication failed for {}@{}", user, host))
    }

    #[allow(dead_code)]
    pub fn exec(&self, command: &str) -> Result<String> {
        let mut channel = self
            .session
            .channel_session()
            .context("Failed to open SSH channel")?;
        channel.exec(command).context("Failed to exec command")?;

        let mut buf = Vec::new();
        channel
            .read_to_end(&mut buf)
            .context("Failed to read command output")?;
        channel.wait_close().ok();

        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    /// Execute a command and call `callback` for each line of output as it arrives.
    /// Reads in 4KB chunks to avoid relying on BufReader + dyn Read coercion edge cases
    /// with the ssh2 channel. Each complete line triggers the callback.
    pub fn exec_streamed<F>(&self, command: &str, mut callback: F) -> Result<()>
    where
        F: FnMut(&str),
    {
        use std::io::Read;

        let mut channel = self
            .session
            .channel_session()
            .context("Failed to open SSH channel")?;
        channel.exec(command).context("Failed to exec command")?;

        let mut buf = Vec::<u8>::with_capacity(65536);
        let mut chunk = [0u8; 4096];

        loop {
            match channel.read(&mut chunk) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    // Dispatch all complete lines
                    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                        let raw = &buf[..pos];
                        // strip trailing \r if present
                        let line_bytes = if raw.ends_with(b"\r") { &raw[..raw.len() - 1] } else { raw };
                        if let Ok(s) = std::str::from_utf8(line_bytes) {
                            if !s.is_empty() {
                                callback(s);
                            }
                        }
                        buf.drain(..=pos);
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // Shouldn't happen in blocking mode, but avoid spinning
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(e) => {
                    // Log unexpected read errors so we know why streaming stopped early.
                    // Only log if we received some data (skip empty-channel / EOF races).
                    if !buf.is_empty() || e.kind() != std::io::ErrorKind::Other {
                        use std::io::Write;
                        use std::os::unix::fs::OpenOptionsExt;
                        let log_path = {
                            let base = std::env::var("XDG_DATA_HOME")
                                .map(std::path::PathBuf::from)
                                .unwrap_or_else(|_| {
                                    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                                    std::path::PathBuf::from(home).join(".local/share")
                                });
                            let dir = base.join("booker-search");
                            let _ = std::fs::create_dir_all(&dir);
                            dir.join("ssh-exec.log")
                        };
                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true)
                            .mode(0o600).open(log_path) {
                            let _ = writeln!(f, "exec_streamed '{}': read error after {} bytes buffered: {:?} ({})",
                                command, buf.len(), e.kind(), e);
                        }
                    }
                    break;
                }
            }
        }

        // Flush any final partial line (no trailing newline)
        if !buf.is_empty() {
            let line_bytes = if buf.ends_with(b"\r\n") { &buf[..buf.len()-2] }
                            else if buf.ends_with(b"\n") { &buf[..buf.len()-1] }
                            else { &buf[..] };
            if let Ok(s) = std::str::from_utf8(line_bytes) {
                if !s.is_empty() {
                    callback(s);
                }
            }
        }

        channel.wait_close().ok();
        Ok(())
    }

    pub fn list_files_streamed<F>(&self, path: &str, callback: F) -> Result<()>
    where
        F: FnMut(&str),
    {
        let cmd = format!(
            "find {} \\( -name '.cache' -o -name 'node_modules' -o -name '.git' -o -name 'Trash' -o -name '__pycache__' \\) -prune -o -type f -print 2>/dev/null",
            shell_escape(path)
        );
        self.exec_streamed(&cmd, callback)
    }

    pub fn list_dirs_streamed<F>(&self, path: &str, callback: F) -> Result<()>
    where
        F: FnMut(&str),
    {
        let cmd = format!(
            "find {} -mindepth 1 \\( -name '.cache' -o -name 'node_modules' -o -name '.git' -o -name 'Trash' -o -name '__pycache__' \\) -prune -o -mindepth 1 -type d -print 2>/dev/null",
            shell_escape(path)
        );
        self.exec_streamed(&cmd, callback)
    }

    /// Grep files on the remote, calling `callback` for each result as it arrives.
    /// Format: callback(filepath, lineno, matched_line).
    pub fn grep_files_streamed<F>(&self, path: &str, pattern: &str, mut callback: F) -> Result<()>
    where
        F: FnMut(String, u64, String),
    {
        let cmd = format!(
            "grep -rFn {} {} 2>/dev/null | head -500",
            shell_escape(pattern),
            shell_escape(path)
        );
        self.exec_streamed(&cmd, |line| {
            // format: filename:lineno:content
            let parts: Vec<&str> = line.splitn(3, ':').collect();
            if parts.len() >= 3 {
                let file = parts[0].to_string();
                let lineno: u64 = parts[1].parse().unwrap_or(0);
                let content = parts[2].to_string();
                callback(file, lineno, content);
            }
        })
    }
}

pub fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
