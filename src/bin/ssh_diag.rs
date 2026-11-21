/// SSH diagnostic tool — run as: ssh_diag HOST PORT USER PASSWORD
/// Writes step-by-step output showing exactly where the connection fails.
use ssh2::{KeyboardInteractivePrompt, Prompt, Session};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

struct PasswordHandler(String);
impl KeyboardInteractivePrompt for PasswordHandler {
    fn prompt(&mut self, _u: &str, _i: &str, prompts: &[Prompt<'_>]) -> Vec<String> {
        vec![self.0.clone(); prompts.len()]
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("Usage: ssh_diag HOST PORT USER PASSWORD");
        std::process::exit(1);
    }
    let host = &args[1];
    let port: u16 = args[2].parse().expect("PORT must be a number");
    let user = &args[3];
    let password = &args[4];

    let addr = format!("{}:{}", host, port);
    println!("=== SSH Diagnostic ===");
    println!("Target: {}", addr);
    println!("User:   {}", user);
    println!();

    // Step 1: TCP connect
    print!("[1] TCP connect to {} ... ", addr);
    let _ = std::io::stdout().flush();
    let tcp = match TcpStream::connect_timeout(
        &addr.parse().expect("invalid address"),
        Duration::from_secs(10),
    ) {
        Ok(t) => { println!("OK"); t }
        Err(e) => { println!("FAILED: {}", e); std::process::exit(1); }
    };

    // Step 2: Peek at SSH banner (server sends it immediately on connect)
    print!("[2] Reading SSH banner ... ");
    let _ = std::io::stdout().flush();
    let mut banner_buf = [0u8; 256];
    // peek without consuming — tcp is still usable after this
    match tcp.peek(&mut banner_buf) {
        Ok(n) if n > 0 => {
            let banner = String::from_utf8_lossy(&banner_buf[..n]);
            let first_line = banner.lines().next().unwrap_or("(empty)");
            println!("\"{}\"", first_line.trim());
        }
        Ok(_) => println!("(empty — server sent nothing yet)"),
        Err(e) => println!("peek failed: {} (non-fatal)", e),
    }

    // Step 3: libssh2 session + handshake
    print!("[3] Creating libssh2 session ... ");
    let _ = std::io::stdout().flush();
    let mut session = match Session::new() {
        Ok(s) => { println!("OK"); s }
        Err(e) => { println!("FAILED: {}", e); std::process::exit(1); }
    };

    session.set_tcp_stream(tcp);
    session.set_timeout(15000); // 15s timeout for handshake

    print!("[4] SSH handshake ... ");
    let _ = std::io::stdout().flush();
    if let Err(e) = session.handshake() {
        println!("FAILED: {}", e);
        println!();
        println!("Handshake failed. Possible causes:");
        println!("  - Server requires algorithms libssh2 doesn't support");
        println!("  - Port is not actually running SSH (wrong port?)");
        println!("  - Server has strict AllowUsers/DenyUsers/Match rules that block before negotiation");
        println!();
        println!("Run: ssh -vv -p {} {}@{}", port, user, host);
        println!("     to see what the system ssh client negotiates.");
        std::process::exit(1);
    }
    println!("OK");

    // Step 5: What did server offer?
    print!("[5] Server host key fingerprint ... ");
    let _ = std::io::stdout().flush();
    match session.host_key_hash(ssh2::HashType::Sha256) {
        Some(hash) => {
            let b64: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
            println!("SHA256:{}", b64);
        }
        None => println!("(unavailable)"),
    }

    // Step 6: What auth methods does server offer?
    print!("[6] Server auth methods ... ");
    let _ = std::io::stdout().flush();
    match session.auth_methods(user) {
        Ok(methods) => println!("{}", methods),
        Err(e) => println!("error querying: {}", e),
    }

    // Step 7: Try SSH agent
    print!("[7] SSH agent auth ... ");
    let _ = std::io::stdout().flush();
    let mut agent_tried = false;
    if let Ok(mut agent) = session.agent() {
        if agent.connect().is_ok() && agent.list_identities().is_ok() {
            for identity in agent.identities().unwrap_or_default() {
                agent_tried = true;
                if agent.userauth(user, &identity).is_ok() && session.authenticated() {
                    println!("OK (via agent key: {})", identity.comment());
                    run_test_command(&session);
                    return;
                }
            }
        }
    }
    if !agent_tried {
        println!("skipped (no agent or no identities)");
    } else {
        println!("no agent key worked");
    }

    // Step 8: Password auth
    print!("[8] Password auth ... ");
    let _ = std::io::stdout().flush();
    match session.userauth_password(user, password) {
        Ok(_) if session.authenticated() => {
            println!("OK");
            run_test_command(&session);
            return;
        }
        Ok(_) => println!("returned OK but not authenticated"),
        Err(e) => println!("FAILED: {}", e),
    }

    // Step 9: Keyboard-interactive
    print!("[9] Keyboard-interactive auth ... ");
    let _ = std::io::stdout().flush();
    match session.userauth_keyboard_interactive(user, &mut PasswordHandler(password.clone())) {
        Ok(_) if session.authenticated() => {
            println!("OK");
            run_test_command(&session);
            return;
        }
        Ok(_) => println!("returned OK but not authenticated"),
        Err(e) => println!("FAILED: {}", e),
    }

    println!();
    println!("All auth methods failed. Server available methods: {}",
        session.auth_methods(user).unwrap_or("unknown"));
    std::process::exit(1);
}

fn run_test_command(session: &Session) {
    println!();
    print!("[10] Running 'echo hello' ... ");
    let _ = std::io::stdout().flush();
    let mut channel = match session.channel_session() {
        Ok(c) => c,
        Err(e) => { println!("channel failed: {}", e); return; }
    };
    if let Err(e) = channel.exec("echo hello") {
        println!("exec failed: {}", e);
        return;
    }
    let mut out = String::new();
    let _ = channel.read_to_string(&mut out);
    channel.wait_close().ok();
    println!("got: {:?}", out.trim());
    println!();
    println!("=== Connection fully working! ===");
}
