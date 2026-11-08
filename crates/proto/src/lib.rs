use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DirectiveMode {
    Block,
    Warn,
    Owner,
    MinPrio,
    PrioFloor,
    Pause,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Directive {
    pub id: u64,
    pub text: String,
    pub nodes: Vec<String>,
    pub mode: DirectiveMode,
    pub owner: Option<String>,
    pub prio: Option<u8>,
    pub pause_target: Option<String>,
    pub from_ts: Option<i64>,
    pub until_ts: i64,
    pub announced_start: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerState {
    pub loop_id: String,
    pub title: String,
    pub pid: u32,
    pub role: Option<String>,
    pub every_secs: Option<u64>,
    pub paused: bool,
    pub needs_you: bool,
    pub last_tick_ts: i64,
    pub next_tick_ts: Option<i64>,
    pub socket_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum ControlRequest {
    Tick,
    Pause,
    Resume,
    Restart { reason: String },
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlResponse {
    pub ok: bool,
    pub message: String,
    pub state: Option<RunnerState>,
}

pub fn parse_duration_secs(input: &str) -> Option<u64> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let (num_str, mult) = if let Some(v) = s.strip_suffix('s') {
        (v, 1)
    } else if let Some(v) = s.strip_suffix('m') {
        (v, 60)
    } else if let Some(v) = s.strip_suffix('h') {
        (v, 3600)
    } else if let Some(v) = s.strip_suffix('d') {
        (v, 86400)
    } else {
        (s, 60)
    };
    num_str.trim().parse::<u64>().ok().map(|n| n * mult)
}
