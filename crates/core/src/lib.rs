use serde::Serialize;
use std::collections::HashMap;
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
    home_dir().join(".local/state/latchd-up")
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

/// Location of the `key = value` config file. `LATCH_CONFIG_FILE` overrides;
/// otherwise the first existing `config` under the latchd / latchboard config
/// dirs wins (defaulting to the latchd one).
pub fn config_file_path() -> PathBuf {
    if let Ok(val) = env::var("LATCH_CONFIG_FILE") {
        return PathBuf::from(val);
    }
    let primary = latchd_config_dir().join("config");
    if primary.exists() {
        return primary;
    }
    let secondary = latchboard_config_dir().join("config");
    if secondary.exists() {
        return secondary;
    }
    primary
}

/// Parse `key = value` lines. Blank lines and `#` comments are skipped, keys
/// are lowercased, and surrounding quotes on values are dropped.
pub fn parse_config(raw: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
        out.insert(k.trim().to_lowercase(), v.to_string());
    }
    out
}

pub fn load_config() -> HashMap<String, String> {
    fs::read_to_string(config_file_path())
        .map(|raw| parse_config(&raw))
        .unwrap_or_default()
}

pub fn config_flag(value: &str) -> bool {
    matches!(
        value.trim().to_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_lines() {
        let cfg = parse_config("# c\n\nAgent_Model = fable\nagent_effort=\"low\"\nbogus\nagent_fullscreen = true\n");
        assert_eq!(cfg.len(), 3);
        assert_eq!(cfg["agent_model"], "fable");
        assert_eq!(cfg["agent_effort"], "low");
        assert!(config_flag(&cfg["agent_fullscreen"]));
        assert!(!config_flag("off"));
    }
}
