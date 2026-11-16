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

/// Parsed config file: settings that apply to every tick, plus the overrides
/// from each `[tick-name]` section.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Config {
    pub global: HashMap<String, String>,
    pub ticks: HashMap<String, HashMap<String, String>>,
}

/// Settings a `[tick]` section may name without the `agent_` prefix.
const TICK_KEYS: [&str; 4] = ["model", "effort", "mcp_config", "fullscreen"];

impl Config {
    /// Parse `key = value` lines. Blank lines and `#` comments are skipped,
    /// keys are lowercased, and surrounding quotes on values are dropped.
    /// Lines after a `[name]` header belong to that tick; there `model`,
    /// `effort`, `mcp_config` and `fullscreen` are short for `agent_model` etc.
    pub fn parse(raw: &str) -> Config {
        let mut cfg = Config::default();
        let mut section: Option<String> = None;
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let name = name.trim().to_string();
                cfg.ticks.entry(name.clone()).or_default();
                section = Some(name);
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let mut key = k.trim().to_lowercase();
            let value = v.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
            match &section {
                Some(name) => {
                    if TICK_KEYS.contains(&key.as_str()) {
                        key = format!("agent_{}", key);
                    }
                    cfg.ticks.entry(name.clone()).or_default().insert(key, value);
                }
                None => {
                    cfg.global.insert(key, value);
                }
            }
        }
        cfg
    }

    /// The settings in force for one tick: the global ones overlaid with the
    /// first section matching any of `names` (e.g. tick id, then file stem).
    pub fn for_tick(&self, names: &[&str]) -> HashMap<String, String> {
        let mut out = self.global.clone();
        if let Some(section) = names.iter().find_map(|n| self.ticks.get(*n)) {
            out.extend(section.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        out
    }
}

/// Global settings only; see [`Config::parse`] for the format.
pub fn parse_config(raw: &str) -> HashMap<String, String> {
    Config::parse(raw).global
}

pub fn load_config() -> Config {
    fs::read_to_string(config_file_path())
        .map(|raw| Config::parse(&raw))
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

    #[test]
    fn tick_sections_override_globals() {
        let cfg = Config::parse(
            "agent_model = fable\nagent_effort = low\n\n[incident]\nmodel = opus\nMCP_Config = a.json\n\n[10-storage-scrub]\nagent_effort = high\nother = x\n",
        );
        assert_eq!(cfg.global.len(), 2);

        let incident = cfg.for_tick(&["incident", "99-incident"]);
        assert_eq!(incident["agent_model"], "opus");
        assert_eq!(incident["agent_effort"], "low");
        assert_eq!(incident["agent_mcp_config"], "a.json");

        // matched by file stem when the tick id has no section
        let scrub = cfg.for_tick(&["storage-scrub", "10-storage-scrub"]);
        assert_eq!(scrub["agent_model"], "fable");
        assert_eq!(scrub["agent_effort"], "high");
        assert_eq!(scrub["other"], "x");

        // a tick without a section gets the globals unchanged
        assert_eq!(cfg.for_tick(&["telemetry"]), cfg.global);
    }
}
