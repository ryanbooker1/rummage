use std::sync::atomic::{AtomicU8, Ordering};
static DEBUG_LEVEL: AtomicU8 = AtomicU8::new(0);

pub fn set_level(level: u8) {
    DEBUG_LEVEL.store(level, Ordering::Relaxed);
}

pub fn level() -> u8 {
    DEBUG_LEVEL.load(Ordering::Relaxed)
}

pub fn log(level: u8, msg: &str) {
    if level > DEBUG_LEVEL.load(Ordering::Relaxed) { return; }
    use std::io::Write;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let prefix = match level {
        1 => "WARN",
        2 => "INFO",
        3 => "DEBUG",
        _ => "TRACE",
    };
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
        dir.join("debug.log")
    };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true)
        .mode(0o600).open(log_path)
    {
        let _ = writeln!(f, "[{}] {} {}", ts, prefix, msg);
    }
}
