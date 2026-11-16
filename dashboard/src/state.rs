use latch_core::{handover_state_dir, latchd_state_dir, latchboard_state_dir};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimEntry {
    pub node: String,
    pub holder: String,
    pub prio: u8,
    /// Held by an incident tick (off-limits to everyone else).
    #[serde(default)]
    pub incident: bool,
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

/// One unexpired record from `directives.json`. Maintenance windows are
/// directives whose text starts with `maintenance:`.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectiveEntry {
    pub id: u64,
    pub text: String,
    pub nodes: Vec<String>,
    pub mode: String,
    pub from_ts: Option<i64>,
    pub until_ts: i64,
    pub synced: bool,
}

impl DirectiveEntry {
    pub fn is_maintenance(&self) -> bool {
        self.text.starts_with("maintenance:")
    }

    pub fn is_upcoming(&self, now: i64) -> bool {
        self.from_ts.is_some_and(|from| from > now)
    }
}

/// Read the directive records that have not ended yet, skipping anything
/// malformed rather than failing.
pub fn parse_directives(raw: &str, now: i64) -> Vec<DirectiveEntry> {
    let Ok(records) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    let mut out: Vec<DirectiveEntry> = records
        .iter()
        .filter_map(|r| {
            let until_ts = r["until_ts"].as_i64()?;
            if until_ts <= now {
                return None;
            }
            Some(DirectiveEntry {
                id: r["id"].as_u64().unwrap_or(0),
                text: r["text"].as_str().unwrap_or("").to_string(),
                nodes: r["nodes"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|n| n.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
                mode: r["mode"].as_str().unwrap_or("?").to_string(),
                from_ts: r["from_ts"].as_i64(),
                until_ts,
                synced: r["sync_name"].is_string(),
            })
        })
        .collect();
    out.sort_by_key(|d| d.id);
    out
}

#[derive(Debug, Default)]
pub struct Snapshot {
    pub claims: Vec<ClaimEntry>,
    pub rollouts: Vec<RolloutEntry>,
    pub runners: Vec<serde_json::Value>,
    pub notices: Vec<serde_json::Value>,
    pub directives: Vec<DirectiveEntry>,
    /// (node, waiter) pairs, first come first within each node.
    pub waiters: Vec<(String, String)>,
}

impl Snapshot {
    /// Who is waiting for a node, in line order.
    pub fn waiters_for(&self, node: &str) -> Vec<&str> {
        self.waiters
            .iter()
            .filter(|(n, _)| n == node)
            .map(|(_, w)| w.as_str())
            .collect()
    }

    /// Nodes somebody waits for although nobody holds them.
    pub fn free_nodes_with_waiters(&self) -> Vec<&str> {
        let mut nodes: Vec<&str> = self
            .waiters
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|n| !self.claims.iter().any(|c| c.node == *n))
            .collect();
        nodes.dedup();
        nodes
    }

    /// Tick ids in the order the Runners panel numbers them.
    pub fn runner_ids(&self) -> Vec<String> {
        self.runners
            .iter()
            .filter_map(|r| r["tick_id"].as_str().map(String::from))
            .collect()
    }

    pub fn maintenance(&self) -> Vec<&DirectiveEntry> {
        self.directives.iter().filter(|d| d.is_maintenance()).collect()
    }

    pub fn plain_directives(&self) -> Vec<&DirectiveEntry> {
        self.directives.iter().filter(|d| !d.is_maintenance()).collect()
    }
}

pub fn load_snapshot() -> Snapshot {
    let mut snap = Snapshot::default();

    let db_path = latchd_state_dir().join("claims.db");
    if let Ok(conn) = Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
        // A database written by an older latchd has no `incident` column.
        let stmt = conn
            .prepare("SELECT node, holder, prio, incident FROM claims ORDER BY node")
            .or_else(|_| conn.prepare("SELECT node, holder, prio, 0 FROM claims ORDER BY node"));
        if let Ok(mut stmt) = stmt {
            if let Ok(rows) = stmt.query_map([], |r| {
                Ok(ClaimEntry {
                    node: r.get(0)?,
                    holder: r.get(1)?,
                    prio: r.get(2)?,
                    incident: r.get::<_, i64>(3)? != 0,
                })
            }) {
                snap.claims = rows.filter_map(Result::ok).collect();
            }
        }
        if let Ok(mut stmt) =
            conn.prepare("SELECT node, waiter FROM queue ORDER BY node, enqueued_at, rowid")
        {
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))) {
                snap.waiters = rows.filter_map(Result::ok).collect();
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
            if e.path().extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            if let Ok(txt) = fs::read_to_string(e.path()) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&txt) {
                    snap.runners.push(val);
                }
            }
        }
    }

    // Stable order: `/shutdown N` refers to the Nth runner listed.
    snap.runners.retain(|r| r["tick_id"].is_string());
    snap.runners
        .sort_by(|a, b| a["tick_id"].as_str().cmp(&b["tick_id"].as_str()));

    if let Ok(txt) = fs::read_to_string(handover_state_dir().join("directives.json")) {
        snap.directives = parse_directives(&txt, latch_core::now_epoch());
    }

    let notices_path = handover_state_dir().join("notices.json");
    if let Ok(txt) = fs::read_to_string(notices_path) {
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&txt) {
            snap.notices = arr;
        }
    }

    snap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directives_are_parsed_leniently() {
        let raw = r#"[
            {"id": 2, "text": "maintenance: PSU swap", "nodes": ["ab1001"], "mode": "block",
             "from_ts": 500, "until_ts": 900},
            {"id": 1, "text": "Freeze", "nodes": ["ab10*", 7], "mode": "block", "until_ts": 400,
             "sync_name": "decom", "unknown_field": true},
            {"id": 3, "text": "Expired", "nodes": [], "mode": "warn", "until_ts": 100},
            {"id": 4, "text": "no end time"},
            "garbage"
        ]"#;
        let d = parse_directives(raw, 200);
        assert_eq!(d.iter().map(|x| x.id).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(d[0].nodes, vec!["ab10*"]);
        assert!(d[0].synced && !d[0].is_maintenance() && !d[0].is_upcoming(200));
        assert!(d[1].is_maintenance() && d[1].is_upcoming(200) && !d[1].is_upcoming(600));

        assert!(parse_directives("not json", 0).is_empty());
        assert!(parse_directives("{}", 0).is_empty());
    }
}
