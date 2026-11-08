use serde::Serialize;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

pub fn latchd_state_dir() -> PathBuf {
    if let Ok(val) = env::var("LATCHD_DIR") {
        return PathBuf::from(val);
    }
    home_dir().join(".local/state/latchd")
}

pub fn handover_state_dir() -> PathBuf {
    if let Ok(val) = env::var("LATCH_HANDOVER_DIR") {
        return PathBuf::from(val);
    }
    home_dir().join(".local/state/morning-startup")
}

pub fn latchboard_state_dir() -> PathBuf {
    if let Ok(val) = env::var("LATCHBOARD_STATE_DIR") {
        return PathBuf::from(val);
    }
    home_dir().join(".local/state/latchboard")
}

pub fn latchboard_config_dir() -> PathBuf {
    if let Ok(val) = env::var("LATCHBOARD_CONFIG") {
        return PathBuf::from(val);
    }
    home_dir().join(".config/latchboard")
}

pub fn latchd_config_dir() -> PathBuf {
    if let Ok(val) = env::var("LATCHD_CONFIG") {
        return PathBuf::from(val);
    }
    home_dir().join(".config/latchd")
}

pub fn ensure_dir(path: &PathBuf) -> std::io::Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)?;
    }
    Ok(())
}

pub fn append_jsonl<T: Serialize>(path: &PathBuf, record: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(record)?;
    writeln!(file, "{}", line)
}

pub fn is_pid_alive(pid: u32) -> bool {
    if env::var("LATCH_NOTE_NO_PROCS").as_deref() == Ok("1") {
        return true;
    }
    unsafe { libc::kill(pid as i32, 0) == 0 }
}
