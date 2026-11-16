use latch_core::{append_jsonl, ensure_dir, handover_state_dir, latchd_state_dir, now_epoch};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;
use std::fs;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

/// A holder that has been asked to hand a host over gets this long to
/// `latchd reply` before the question goes to the operator.
pub const ASK_TIMEOUT_SECS: i64 = 15 * 60;
/// The first waiter in line gets this long to claim a host that became free
/// before the next waiter gets its turn.
pub const OFFER_SECS: i64 = 10 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimRow {
    pub node: String,
    pub holder: String,
    pub prio: u8,
    pub claimed_at: i64,
    pub last_used_at: i64,
    pub warned_at: Option<i64>,
    pub known_node: bool,
    /// Held by an incident tick: off-limits to every other session.
    pub incident: bool,
}

/// One waiter in a host's first-come-first-served queue, together with the
/// state of its takeover request to the current holder (only the first in
/// line negotiates).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueRow {
    pub node: String,
    pub waiter: String,
    pub base_prio: u8,
    pub enqueued_at: i64,
    pub reason: Option<String>,
    /// When the holder named in `asked_holder` was asked to hand over.
    pub asked_at: Option<i64>,
    pub asked_holder: Option<String>,
    /// The holder's answer: `yes` or `no`.
    pub reply: Option<String>,
    pub eta_ts: Option<i64>,
    pub replied_at: Option<i64>,
    /// When the operator was asked because the holder did not answer in time.
    pub escalated_at: Option<i64>,
    /// When this waiter was told the host is free and it is first in line.
    pub offered_at: Option<i64>,
}

const CLAIM_COLS: &str =
    "node, holder, prio, claimed_at, last_used_at, warned_at, known_node, incident";
const QUEUE_COLS: &str = "node, waiter, base_prio, enqueued_at, reason, asked_at, asked_holder, \
     reply, eta_ts, replied_at, escalated_at, offered_at";

fn claim_from_row(r: &rusqlite::Row) -> rusqlite::Result<ClaimRow> {
    Ok(ClaimRow {
        node: r.get(0)?,
        holder: r.get(1)?,
        prio: r.get(2)?,
        claimed_at: r.get(3)?,
        last_used_at: r.get(4)?,
        warned_at: r.get(5)?,
        known_node: r.get::<_, i64>(6)? != 0,
        incident: r.get::<_, i64>(7)? != 0,
    })
}

fn queue_from_row(r: &rusqlite::Row) -> rusqlite::Result<QueueRow> {
    Ok(QueueRow {
        node: r.get(0)?,
        waiter: r.get(1)?,
        base_prio: r.get(2)?,
        enqueued_at: r.get(3)?,
        reason: r.get(4)?,
        asked_at: r.get(5)?,
        asked_holder: r.get(6)?,
        reply: r.get(7)?,
        eta_ts: r.get(8)?,
        replied_at: r.get(9)?,
        escalated_at: r.get(10)?,
        offered_at: r.get(11)?,
    })
}

/// Add a column to a table created by an earlier version.
fn add_column_if_missing(conn: &Connection, table: &str, column: &str, decl: &str) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
    let present = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == column);
    if !present {
        conn.execute_batch(&format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, decl))?;
    }
    Ok(())
}

pub fn open_db() -> rusqlite::Result<Connection> {
    let dir = latchd_state_dir();
    ensure_dir(&dir).ok();
    let db_path = dir.join("claims.db");
    let conn = Connection::open(db_path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
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
    add_column_if_missing(&conn, "claims", "incident", "INTEGER NOT NULL DEFAULT 0")?;
    for (column, decl) in [
        ("reason", "TEXT"),
        ("asked_at", "INTEGER"),
        ("asked_holder", "TEXT"),
        ("reply", "TEXT"),
        ("eta_ts", "INTEGER"),
        ("replied_at", "INTEGER"),
        ("escalated_at", "INTEGER"),
        ("offered_at", "INTEGER"),
    ] {
        add_column_if_missing(&conn, "queue", column, decl)?;
    }
    Ok(conn)
}

pub fn get_claim(conn: &Connection, node: &str) -> Result<Option<ClaimRow>, String> {
    conn.query_row(
        &format!("SELECT {} FROM claims WHERE node = ?1", CLAIM_COLS),
        params![node],
        claim_from_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn load_claims(conn: &Connection) -> Result<Vec<ClaimRow>, String> {
    let mut stmt = conn
        .prepare(&format!("SELECT {} FROM claims ORDER BY node", CLAIM_COLS))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], claim_from_row)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

/// A host's waiters, first come first.
pub fn queue_for(conn: &Connection, node: &str) -> Result<Vec<QueueRow>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM queue WHERE node = ?1 ORDER BY enqueued_at, rowid",
            QUEUE_COLS
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![node], queue_from_row)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

/// Every waiter, grouped by host and first come first within each.
pub fn load_queue(conn: &Connection) -> Result<Vec<QueueRow>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM queue ORDER BY node, enqueued_at, rowid",
            QUEUE_COLS
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], queue_from_row)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

/// `90` -> `1m`, `5400` -> `1h30m`.
pub fn fmt_span(secs: i64) -> String {
    let mins = secs.max(0) / 60;
    if mins < 60 {
        format!("{}m", mins)
    } else {
        format!("{}h{:02}m", mins / 60, mins % 60)
    }
}

/// Drop a note into a tick's inbox (`latch-note inbox` reads these).
pub fn send_note(to: &str, node: &str, note: &str) {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = handover_state_dir().join("handover").join(to);
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let ts = now_epoch();
    let payload = json!({"ts": ts, "from": "latchd", "to": to, "nodes": [node], "note": note});
    let name = format!(
        "{}-latchd-{}-{}.json",
        ts,
        process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    if let Ok(text) = serde_json::to_string_pretty(&payload) {
        let _ = fs::write(dir.join(name), text);
    }
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
                let holder = val.get("tick_id").and_then(|v| v.as_str()).unwrap_or("");
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

/// Forget any takeover request state on a host's queue: its holder changed
/// or it became free, so earlier questions and answers no longer apply.
fn reset_asks(conn: &Connection, node: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE queue SET asked_at = NULL, asked_holder = NULL, reply = NULL, eta_ts = NULL,
                          replied_at = NULL, escalated_at = NULL
         WHERE node = ?1",
        params![node],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// If the host is free, tell the first waiter in line that it is theirs to
/// claim. Returns that waiter when it was told just now.
pub fn offer_to_head(conn: &Connection, node: &str) -> Result<Option<String>, String> {
    if get_claim(conn, node)?.is_some() {
        return Ok(None);
    }
    let Some(head) = queue_for(conn, node)?.into_iter().next() else {
        return Ok(None);
    };
    if head.offered_at.is_some() {
        return Ok(None);
    }
    conn.execute(
        "UPDATE queue SET offered_at = ?1 WHERE node = ?2 AND waiter = ?3",
        params![now_epoch(), node, head.waiter],
    )
    .map_err(|e| e.to_string())?;
    send_note(
        &head.waiter,
        node,
        &format!(
            "{} is free and you are first in line: run `latchd claim {}` within {} or lose your turn",
            node,
            node,
            fmt_span(OFFER_SECS)
        ),
    );
    log_event("queue_offer", json!({"node": node, "waiter": head.waiter}));
    Ok(Some(head.waiter))
}

/// If the host is held by a standard tick, make sure the first waiter in line
/// has asked that holder to hand it over. Returns (holder, waiter) when the
/// question was sent just now. Incident ticks are never asked.
pub fn ensure_ask(conn: &Connection, node: &str) -> Result<Option<(String, String)>, String> {
    let Some(claim) = get_claim(conn, node)? else {
        return Ok(None);
    };
    if claim.incident {
        return Ok(None);
    }
    let Some(head) = queue_for(conn, node)?.into_iter().next() else {
        return Ok(None);
    };
    let now = now_epoch();
    let asked_this_holder = head.asked_holder.as_deref() == Some(claim.holder.as_str());
    // After a "no", ask again once the holder's ETA has passed (or, with no
    // ETA, after the usual timeout).
    let retry_due = head.reply.as_deref() == Some("no")
        && now >= head.eta_ts.unwrap_or(head.replied_at.unwrap_or(now) + ASK_TIMEOUT_SECS);
    if asked_this_holder && !retry_due {
        return Ok(None);
    }
    conn.execute(
        "UPDATE queue SET asked_at = ?1, asked_holder = ?2, reply = NULL, eta_ts = NULL,
                          replied_at = NULL, escalated_at = NULL, offered_at = NULL
         WHERE node = ?3 AND waiter = ?4",
        params![now, claim.holder, node, head.waiter],
    )
    .map_err(|e| e.to_string())?;
    let why = head
        .reason
        .as_deref()
        .map(|r| format!(" ({})", r))
        .unwrap_or_default();
    send_note(
        &claim.holder,
        node,
        &format!(
            "{} is waiting for {}{}. Answer with `latchd reply {} yes` to hand it over now, \
             `latchd reply {} yes --eta 20m` to promise it, or `latchd reply {} no --eta 1h`.",
            head.waiter, node, why, node, node, node
        ),
    );
    log_event(
        "takeover_ask",
        json!({"node": node, "waiter": head.waiter, "holder": claim.holder, "reason": head.reason}),
    );
    Ok(Some((claim.holder, head.waiter)))
}

pub fn claim_node(
    conn: &Connection,
    node: &str,
    holder: &str,
    prio: u8,
    known_node: bool,
    incident: bool,
) -> Result<String, String> {
    let now = now_epoch();

    if let Some(row) = get_claim(conn, node)? {
        if row.holder == holder {
            let new_prio = row.prio.max(prio);
            conn.execute(
                "UPDATE claims SET last_used_at = ?1, prio = ?2, warned_at = NULL, incident = ?3
                 WHERE node = ?4",
                params![now, new_prio, (row.incident || incident) as i64, node],
            )
            .map_err(|e| e.to_string())?;
            return Ok(format!("renewed {} for {} (p{})", node, holder, new_prio));
        }
        if row.incident {
            return Err(format!(
                "refused: {} is held by incident tick {} and is off-limits until it is released",
                node, row.holder
            ));
        }
        let hint = if incident {
            format!("; an incident tick takes it with `latchd take {}`", node)
        } else {
            format!("; run `latchd wait {}` to queue and ask for it", node)
        };
        return Err(format!(
            "refused: {} is held by {} (p{}){}",
            node, row.holder, row.prio, hint
        ));
    }

    // Free host: first come, first served. Incident ticks do not queue.
    if !incident {
        if let Some(head) = queue_for(conn, node)?.into_iter().next() {
            if head.waiter != holder {
                return Err(format!(
                    "refused: {} is free but {} is first in line (waiting {}); run `latchd wait {}` to queue",
                    node,
                    head.waiter,
                    fmt_span(now - head.enqueued_at),
                    node
                ));
            }
        }
    }

    conn.execute(
        "INSERT INTO claims (node, holder, prio, claimed_at, last_used_at, warned_at, known_node, incident)
         VALUES (?1, ?2, ?3, ?4, ?4, NULL, ?5, ?6)",
        params![node, holder, prio, now, known_node as i64, incident as i64],
    )
    .map_err(|e| e.to_string())?;

    conn.execute(
        "DELETE FROM queue WHERE node = ?1 AND waiter = ?2",
        params![node, holder],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("UPDATE queue SET offered_at = NULL WHERE node = ?1", params![node])
        .map_err(|e| e.to_string())?;
    reset_asks(conn, node)?;

    log_event(
        "claim",
        json!({"node": node, "holder": holder, "prio": prio, "known": known_node, "incident": incident}),
    );
    let tag = if incident { " [incident]" } else { "" };
    Ok(format!("claimed {} for {} (p{}){}", node, holder, prio, tag))
}

/// Bookkeeping after a host became free: earlier takeover questions are moot
/// and the first waiter gets its turn.
pub fn after_release(conn: &Connection, node: &str) -> Result<Option<String>, String> {
    reset_asks(conn, node)?;
    offer_to_head(conn, node)
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
    match after_release(conn, node)? {
        Some(next) => Ok(format!("released {} ({} is next in line and was told)", node, next)),
        None => Ok(format!("released {}", node)),
    }
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{}{}", n, suffix)
}

/// Join a host's queue (first come, first served). The first in line asks the
/// holder to hand the host over; everyone else waits their turn.
pub fn enqueue_wait(
    conn: &Connection,
    node: &str,
    waiter: &str,
    prio: Option<u8>,
    reason: Option<&str>,
) -> Result<String, String> {
    let now = now_epoch();
    let claim = get_claim(conn, node)?;
    if claim.as_ref().is_some_and(|c| c.holder == waiter) {
        return Err(format!("{} already holds {}", waiter, node));
    }

    // Queueing again keeps the original place in line.
    let fresh = conn
        .execute(
            "INSERT INTO queue (node, waiter, base_prio, enqueued_at, reason)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(node, waiter) DO NOTHING",
            params![node, waiter, prio.unwrap_or(5), now, reason],
        )
        .map_err(|e| e.to_string())?
        > 0;
    if fresh {
        log_event("wait", json!({"node": node, "waiter": waiter, "reason": reason}));
    } else {
        conn.execute(
            "UPDATE queue SET base_prio = COALESCE(?1, base_prio), reason = COALESCE(?2, reason)
             WHERE node = ?3 AND waiter = ?4",
            params![prio, reason, node, waiter],
        )
        .map_err(|e| e.to_string())?;
    }

    let asked_now = ensure_ask(conn, node)?;
    let offered_now = offer_to_head(conn, node)?;

    let queue = queue_for(conn, node)?;
    let pos = queue.iter().position(|q| q.waiter == waiter).unwrap_or(0);
    let me = &queue[pos];
    let place = format!("queued {} for {} ({} in line)", waiter, node, ordinal(pos + 1));

    let detail = match &claim {
        None if pos == 0 => {
            let _ = offered_now;
            format!("{} is free: run `latchd claim {}` now", node, node)
        }
        None => format!("{} is free and offered to {}", node, queue[0].waiter),
        Some(c) if c.incident => format!(
            "held by incident tick {}, which is never asked to yield: wait for its release",
            c.holder
        ),
        Some(c) if pos > 0 => format!("held by {}; {} is ahead of you", c.holder, queue[pos - 1].waiter),
        Some(c) => match (me.reply.as_deref(), me.eta_ts) {
            _ if asked_now.is_some() => format!(
                "asked {} to hand it over; it has {} to `latchd reply` before the operator is asked",
                c.holder,
                fmt_span(ASK_TIMEOUT_SECS)
            ),
            (Some("yes"), Some(eta)) => format!(
                "{} promised to hand it over in about {}",
                c.holder,
                fmt_span(eta - now)
            ),
            (Some("no"), Some(eta)) if eta > now => format!(
                "{} declined; it expects to be done in about {}",
                c.holder,
                fmt_span(eta - now)
            ),
            (Some("no"), _) => format!("{} declined; it will be asked again later", c.holder),
            _ => format!(
                "already asked {} {} ago; still waiting for its reply",
                c.holder,
                fmt_span(now - me.asked_at.unwrap_or(now))
            ),
        },
    };
    Ok(format!("{}; {}", place, detail))
}

/// The holder's answer to the first waiter in line.
pub fn reply_takeover(
    conn: &Connection,
    node: &str,
    holder: &str,
    yes: bool,
    eta_secs: Option<i64>,
) -> Result<String, String> {
    let now = now_epoch();
    let Some(claim) = get_claim(conn, node)? else {
        return Err(format!("{} is not held by anyone", node));
    };
    if claim.holder != holder {
        return Err(format!("{} is held by {}, not {}", node, claim.holder, holder));
    }
    let head = queue_for(conn, node)?
        .into_iter()
        .next()
        .filter(|h| h.asked_holder.as_deref() == Some(holder));
    let Some(head) = head else {
        return Err(format!("nobody is asking for {}", node));
    };

    let eta_ts = eta_secs.map(|s| now + s);
    if yes && eta_ts.is_none() {
        // Hand over on the spot: the claim moves to the waiter.
        conn.execute(
            "UPDATE claims SET holder = ?1, prio = ?2, claimed_at = ?3, last_used_at = ?3,
                               warned_at = NULL, incident = 0
             WHERE node = ?4",
            params![head.waiter, head.base_prio, now, node],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM queue WHERE node = ?1 AND waiter = ?2",
            params![node, head.waiter],
        )
        .map_err(|e| e.to_string())?;
        reset_asks(conn, node)?;
        send_note(
            &head.waiter,
            node,
            &format!("{} handed {} over to you: it is yours now", holder, node),
        );
        log_event(
            "takeover_granted",
            json!({"node": node, "from": holder, "to": head.waiter}),
        );
        return Ok(format!("handed {} over to {}", node, head.waiter));
    }

    let answer = if yes { "yes" } else { "no" };
    conn.execute(
        "UPDATE queue SET reply = ?1, eta_ts = ?2, replied_at = ?3, escalated_at = NULL
         WHERE node = ?4 AND waiter = ?5",
        params![answer, eta_ts, now, node, head.waiter],
    )
    .map_err(|e| e.to_string())?;
    let eta_text = eta_secs.map(|s| format!(" in about {}", fmt_span(s))).unwrap_or_default();
    let (note, msg) = if yes {
        (
            format!("{} will hand {} over{}; you stay first in line", holder, node, eta_text),
            format!("told {} it gets {}{}; release it when you are done", head.waiter, node, eta_text),
        )
    } else {
        (
            format!(
                "{} will not hand {} over{}; you stay first in line",
                holder,
                node,
                eta_secs.map(|s| format!(" (done in about {})", fmt_span(s))).unwrap_or_default()
            ),
            format!("told {} that {} stays with you{}", head.waiter, node, eta_secs.map(|s| format!(" for about {}", fmt_span(s))).unwrap_or_default()),
        )
    };
    send_note(&head.waiter, node, &note);
    log_event(
        "takeover_reply",
        json!({"node": node, "holder": holder, "waiter": head.waiter, "reply": answer, "eta_ts": eta_ts}),
    );
    Ok(msg)
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

/// An incident tick takes a host. A standard holder loses it without being
/// asked and the queue is bypassed; a host held by another incident tick is
/// off-limits.
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

    let previous = get_claim(conn, node)?;
    if let Some(prev) = &previous {
        if prev.incident && prev.holder != session {
            return Err(format!(
                "refused: {} is held by incident tick {} and is off-limits until it is released",
                node, prev.holder
            ));
        }
    }

    conn.execute(
        "INSERT INTO claims (node, holder, prio, claimed_at, last_used_at, warned_at, known_node, incident)
         VALUES (?1, ?2, 9, ?3, ?3, NULL, 1, 1)
         ON CONFLICT(node) DO UPDATE SET
            holder = excluded.holder,
            prio = 9,
            claimed_at = excluded.claimed_at,
            last_used_at = excluded.last_used_at,
            warned_at = NULL,
            incident = 1",
        params![node, session, now],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM queue WHERE node = ?1 AND waiter = ?2",
        params![node, session],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("UPDATE queue SET offered_at = NULL WHERE node = ?1", params![node])
        .map_err(|e| e.to_string())?;
    reset_asks(conn, node)?;

    let from = previous
        .as_ref()
        .filter(|p| p.holder != session)
        .map(|p| p.holder.clone());
    if let Some(old) = &from {
        send_note(
            old,
            node,
            &format!(
                "incident tick {} took {} from you: stop working on it and do not touch it until it is released",
                session, node
            ),
        );
    }
    log_event(
        "take",
        json!({"node": node, "session": session, "prio": 9, "from": from}),
    );
    match from {
        Some(old) => Ok(format!(
            "incident take: {} claimed by {} at p9 (taken from {})",
            node, session, old
        )),
        None => Ok(format!("incident take: {} claimed by {} at p9", node, session)),
    }
}
