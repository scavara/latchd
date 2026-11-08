use latch_core::{append_jsonl, ensure_dir, handover_state_dir, latchd_state_dir, now_epoch};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimRow {
    pub node: String,
    pub holder: String,
    pub prio: u8,
    pub claimed_at: i64,
    pub last_used_at: i64,
    pub warned_at: Option<i64>,
    pub known_node: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueRow {
    pub node: String,
    pub waiter: String,
    pub base_prio: u8,
    pub enqueued_at: i64,
}

impl QueueRow {
    pub fn effective_prio(&self, now: i64) -> u8 {
        let elapsed_mins = ((now - self.enqueued_at).max(0)) / 60;
        let bonus = (elapsed_mins / 30).min(2) as u8;
        (self.base_prio + bonus).min(9)
    }
}

pub fn open_db() -> rusqlite::Result<Connection> {
    let dir = latchd_state_dir();
    ensure_dir(&dir).ok();
    let db_path = dir.join("claims.db");
    let conn = Connection::open(db_path)?;
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        CREATE TABLE IF NOT EXISTS claims (
            node TEXT PRIMARY KEY,
            holder TEXT NOT NULL,
            prio INTEGER NOT NULL,
            claimed_at INTEGER NOT NULL,
            last_used_at INTEGER NOT NULL,
            warned_at INTEGER,
            known_node INTEGER NOT NULL DEFAULT 1
        );
        CREATE TABLE IF NOT EXISTS queue (
            node TEXT NOT NULL,
            waiter TEXT NOT NULL,
            base_prio INTEGER NOT NULL,
            enqueued_at INTEGER NOT NULL,
            PRIMARY KEY (node, waiter)
        );
        CREATE TABLE IF NOT EXISTS take_grants (
            node TEXT NOT NULL,
            session TEXT NOT NULL,
            granted_at INTEGER NOT NULL,
            PRIMARY KEY (node, session)
        );
        ",
    )?;
    Ok(conn)
}

pub fn log_event(kind: &str, details: serde_json::Value) {
    let path = latchd_state_dir().join("events.jsonl");
    let record = json!({
        "ts": now_epoch(),
        "event": kind,
        "details": details,
    });
    let _ = append_jsonl(&path, &record);
}

pub fn heartbeat_protected_nodes() -> HashSet<(String, String)> {
    let mut set = HashSet::new();
    let peers_dir = handover_state_dir().join("peers");
    let Ok(entries) = fs::read_dir(peers_dir) else {
        return set;
    };
    let now = now_epoch();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                let holder = val.get("loop_id").and_then(|v| v.as_str()).unwrap_or("");
                let expires = val.get("expires_at").and_then(|v| v.as_i64()).unwrap_or(0);
                if expires >= now {
                    if let Some(nodes) = val.get("nodes").and_then(|v| v.as_array()) {
                        for n in nodes.iter().filter_map(|x| x.as_str()) {
                            set.insert((holder.to_string(), n.to_string()));
                        }
                    }
                }
            }
        }
    }
    set
}

pub fn claim_node(
    conn: &Connection,
    node: &str,
    holder: &str,
    prio: u8,
    known_node: bool,
) -> Result<String, String> {
    let now = now_epoch();
    let existing: Option<ClaimRow> = conn
        .query_row(
            "SELECT node, holder, prio, claimed_at, last_used_at, warned_at, known_node FROM claims WHERE node = ?1",
            params![node],
            |r| {
                Ok(ClaimRow {
                    node: r.get(0)?,
                    holder: r.get(1)?,
                    prio: r.get(2)?,
                    claimed_at: r.get(3)?,
                    last_used_at: r.get(4)?,
                    warned_at: r.get(5)?,
                    known_node: r.get::<_, i64>(6)? != 0,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;

    if let Some(row) = existing {
        if row.holder == holder {
            let new_prio = row.prio.max(prio);
            conn.execute(
                "UPDATE claims SET last_used_at = ?1, prio = ?2, warned_at = NULL WHERE node = ?3",
                params![now, new_prio, node],
            )
            .map_err(|e| e.to_string())?;
            return Ok(format!("renewed {} for {} (p{})", node, holder, new_prio));
        } else {
            return Err(format!(
                "refused: {} is held by {} (p{})",
                node, row.holder, row.prio
            ));
        }
    }

    conn.execute(
        "INSERT INTO claims (node, holder, prio, claimed_at, last_used_at, warned_at, known_node)
         VALUES (?1, ?2, ?3, ?4, ?4, NULL, ?5)",
        params![node, holder, prio, now, if known_node { 1 } else { 0 }],
    )
    .map_err(|e| e.to_string())?;

    conn.execute(
        "DELETE FROM queue WHERE node = ?1 AND waiter = ?2",
        params![node, holder],
    )
    .ok();

    log_event(
        "claim",
        json!({"node": node, "holder": holder, "prio": prio, "known": known_node}),
    );
    Ok(format!("claimed {} for {} (p{})", node, holder, prio))
}

pub fn release_node(conn: &Connection, node: &str, holder: Option<&str>) -> Result<String, String> {
    let changed = if let Some(h) = holder {
        conn.execute(
            "DELETE FROM claims WHERE node = ?1 AND holder = ?2",
            params![node, h],
        )
        .map_err(|e| e.to_string())?
    } else {
        conn.execute("DELETE FROM claims WHERE node = ?1", params![node])
            .map_err(|e| e.to_string())?
    };

    if changed == 0 {
        return Err(format!("no matching claim on {}", node));
    }
    log_event("release", json!({"node": node, "holder": holder}));
    Ok(format!("released {}", node))
}

pub fn enqueue_wait(
    conn: &Connection,
    node: &str,
    waiter: &str,
    prio: u8,
) -> Result<String, String> {
    let now = now_epoch();
    conn.execute(
        "INSERT INTO queue (node, waiter, base_prio, enqueued_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(node, waiter) DO UPDATE SET base_prio = excluded.base_prio",
        params![node, waiter, prio, now],
    )
    .map_err(|e| e.to_string())?;

    let mut yield_msg = String::new();
    if let Ok(Some((holder, holder_prio))) = conn
        .query_row(
            "SELECT holder, prio FROM claims WHERE node = ?1",
            params![node],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, u8>(1)?)),
        )
        .optional()
    {
        if prio >= holder_prio.saturating_add(2) {
            yield_msg = format!(" (yield requested from {} at p{})", holder, holder_prio);
            log_event(
                "yield_request",
                json!({"node": node, "waiter": waiter, "waiter_prio": prio, "holder": holder, "holder_prio": holder_prio}),
            );
        }
    }

    log_event("wait", json!({"node": node, "waiter": waiter, "prio": prio}));
    Ok(format!("queued {} for {} at p{}{}", waiter, node, prio, yield_msg))
}

pub fn record_take_grant(conn: &Connection, node: &str, session: &str) -> Result<(), String> {
    let now = now_epoch();
    conn.execute(
        "INSERT INTO take_grants (node, session, granted_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(node, session) DO UPDATE SET granted_at = excluded.granted_at",
        params![node, session, now],
    )
    .map_err(|e| e.to_string())?;
    log_event("take_grant", json!({"node": node, "session": session}));
    Ok(())
}

pub fn take_node_incident(
    conn: &Connection,
    node: &str,
    session: &str,
) -> Result<String, String> {
    let now = now_epoch();
    let granted_at: Option<i64> = conn
        .query_row(
            "SELECT granted_at FROM take_grants WHERE node = ?1 AND session = ?2",
            params![node, session],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let Some(ts) = granted_at else {
        return Err(format!(
            "refused: no prompt grant for {} in session {}",
            node, session
        ));
    };

    if now - ts > 3600 {
        return Err(format!(
            "refused: prompt grant for {} expired (> 60m)",
            node
        ));
    }

    conn.execute(
        "INSERT INTO claims (node, holder, prio, claimed_at, last_used_at, warned_at, known_node)
         VALUES (?1, ?2, 9, ?3, ?3, NULL, 1)
         ON CONFLICT(node) DO UPDATE SET
            holder = excluded.holder,
            prio = 9,
            claimed_at = excluded.claimed_at,
            last_used_at = excluded.last_used_at,
            warned_at = NULL",
        params![node, session, now],
    )
    .map_err(|e| e.to_string())?;

    log_event("take", json!({"node": node, "session": session, "prio": 9}));
    Ok(format!("incident take: {} claimed by {} at p9", node, session))
}
