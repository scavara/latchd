use latch_core::{handover_state_dir, latchd_state_dir, latchboard_state_dir};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimEntry {
    pub node: String,
    pub holder: String,
    pub prio: u8,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RolloutEntry {
    pub id: i64,
    pub name: String,
    pub owner: String,
    pub phase: String,
    pub state: String,
    pub note: String,
}

#[derive(Debug, Default)]
pub struct Snapshot {
    pub claims: Vec<ClaimEntry>,
    pub rollouts: Vec<RolloutEntry>,
    pub runners: Vec<serde_json::Value>,
    pub notices: Vec<serde_json::Value>,
}

pub fn load_snapshot() -> Snapshot {
    let mut snap = Snapshot::default();

    let db_path = latchd_state_dir().join("claims.db");
    if let Ok(conn) = Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
        if let Ok(mut stmt) = conn.prepare("SELECT node, holder, prio FROM claims ORDER BY node") {
            if let Ok(rows) = stmt.query_map([], |r| {
                Ok(ClaimEntry {
                    node: r.get(0)?,
                    holder: r.get(1)?,
                    prio: r.get(2)?,
                })
            }) {
                snap.claims = rows.filter_map(Result::ok).collect();
            }
        }
    }

    let r_db = handover_state_dir().join("rollouts.db");
    if let Ok(conn) = Connection::open_with_flags(&r_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
        if let Ok(mut stmt) = conn.prepare("SELECT id, name, owner, phase, state, note FROM rollouts ORDER BY id DESC") {
            if let Ok(rows) = stmt.query_map([], |r| {
                Ok(RolloutEntry {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    owner: r.get(2)?,
                    phase: r.get(3)?,
                    state: r.get(4)?,
                    note: r.get(5)?,
                })
            }) {
                snap.rollouts = rows.filter_map(Result::ok).collect();
            }
        }
    }

    let reg_dir = latchboard_state_dir().join("runners");
    if let Ok(entries) = fs::read_dir(reg_dir) {
        for e in entries.flatten() {
            if let Ok(txt) = fs::read_to_string(e.path()) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&txt) {
                    snap.runners.push(val);
                }
            }
        }
    }

    let notices_path = handover_state_dir().join("notices.json");
    if let Ok(txt) = fs::read_to_string(notices_path) {
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&txt) {
            snap.notices = arr;
        }
    }

    snap
}
