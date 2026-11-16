use crate::db::{
    self, after_release, ensure_ask, fmt_span, heartbeat_protected_nodes, log_event, offer_to_head,
    QueueRow, ASK_TIMEOUT_SECS, OFFER_SECS,
};
use crate::directives::prune_expired_directives;
use latch_core::now_epoch;
use rusqlite::{params, Connection};
use serde_json::json;
use std::collections::BTreeSet;
use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Where `latch-note` lives: an explicit override, next to this binary, or on
/// PATH.
fn latch_note_bin() -> PathBuf {
    if let Ok(path) = env::var("LATCH_NOTE_BIN") {
        return PathBuf::from(path);
    }
    env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("latch-note")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("latch-note"))
}

/// Put a question to the operator on a waiter's behalf (`latch-note ask`).
fn ask_operator(waiter: &str, node: &str, question: &str) -> bool {
    Command::new(latch_note_bin())
        .args(["ask", question, node])
        .env("LATCH_SESSION", waiter)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Whether the first waiter's takeover request has gone unanswered (or its
/// promised hand-over has not happened) for long enough to involve the
/// operator. Returns the question to put to them.
fn escalation_due(head: &QueueRow, holder: &str, now: i64) -> Option<String> {
    if head.escalated_at.is_some() || head.asked_holder.as_deref() != Some(holder) {
        return None;
    }
    let why = head
        .reason
        .as_deref()
        .map(|r| format!(" ({})", r))
        .unwrap_or_default();
    match (head.reply.as_deref(), head.asked_at, head.eta_ts) {
        (None, Some(asked), _) if now - asked >= ASK_TIMEOUT_SECS => Some(format!(
            "{} asked {} for {}{} {} ago and got no reply. Should {} hand it over?",
            head.waiter,
            holder,
            head.node,
            why,
            fmt_span(now - asked),
            holder
        )),
        (Some("yes"), _, Some(eta)) if now - eta >= ASK_TIMEOUT_SECS => Some(format!(
            "{} promised {} to {}{} but still holds it {} past its ETA. Should it hand over now?",
            holder,
            head.node,
            head.waiter,
            why,
            fmt_span(now - eta)
        )),
        _ => None,
    }
}

/// Work through each host's first-come-first-served queue: offer free hosts
/// to the first waiter, skip waiters that let their turn pass, send takeover
/// questions, and involve the operator when a holder stays silent.
fn run_queue(conn: &Connection, now: i64, actions: &mut Vec<String>) -> Result<(), String> {
    let nodes: BTreeSet<String> = db::load_queue(conn)?.into_iter().map(|q| q.node).collect();

    for node in nodes {
        let Some(claim) = db::get_claim(conn, &node)? else {
            // Free host: the first in line has OFFER_SECS to claim it.
            if let Some(head) = db::queue_for(conn, &node)?.into_iter().next() {
                if head.offered_at.is_some_and(|t| now - t >= OFFER_SECS) {
                    conn.execute(
                        "DELETE FROM queue WHERE node = ?1 AND waiter = ?2",
                        params![node, head.waiter],
                    )
                    .map_err(|e| e.to_string())?;
                    db::send_note(
                        &head.waiter,
                        &node,
                        &format!(
                            "you did not claim {} within {}, so you lost your place in line",
                            node,
                            fmt_span(OFFER_SECS)
                        ),
                    );
                    log_event("queue_skip", json!({"node": node, "waiter": head.waiter}));
                    actions.push(format!(
                        "{} let its turn for {} pass and left the queue",
                        head.waiter, node
                    ));
                }
            }
            if let Some(next) = offer_to_head(conn, &node)? {
                actions.push(format!("offered free node {} to {}", node, next));
            }
            continue;
        };

        if claim.incident {
            // Incident ticks are never asked to yield; waiters simply wait.
            continue;
        }

        if let Some((holder, waiter)) = ensure_ask(conn, &node)? {
            actions.push(format!("asked {} to hand {} over to {}", holder, node, waiter));
            continue;
        }

        let Some(head) = db::queue_for(conn, &node)?.into_iter().next() else {
            continue;
        };
        if let Some(question) = escalation_due(&head, &claim.holder, now) {
            let reached = ask_operator(&head.waiter, &node, &question);
            conn.execute(
                "UPDATE queue SET escalated_at = ?1 WHERE node = ?2 AND waiter = ?3",
                params![now, node, head.waiter],
            )
            .map_err(|e| e.to_string())?;
            log_event(
                "takeover_escalated",
                json!({
                    "node": node,
                    "waiter": head.waiter,
                    "holder": claim.holder,
                    "operator_asked": reached
                }),
            );
            actions.push(format!(
                "asked the operator about {}: {} is waiting on {}{}",
                node,
                head.waiter,
                claim.holder,
                if reached { "" } else { " (latch-note could not be run)" }
            ));
        }
    }
    Ok(())
}

pub fn run_sweep(conn: &Connection) -> Result<Vec<String>, String> {
    let now = now_epoch();
    let hb_protected = heartbeat_protected_nodes();
    let mut actions = Vec::new();

    for id in prune_expired_directives() {
        log_event("directive_expired", json!({"id": id}));
        actions.push(format!("expired directive #{} removed", id));
    }

    for c in db::load_claims(conn)? {
        let idle_secs = now - c.last_used_at;
        let is_protected = hb_protected.contains(&(c.holder.clone(), c.node.clone()));

        if !c.known_node && idle_secs >= 600 && !is_protected {
            conn.execute("DELETE FROM claims WHERE node = ?1", params![c.node])
                .map_err(|e| e.to_string())?;
            after_release(conn, &c.node)?;
            log_event(
                "sweep_release_unknown",
                json!({"node": c.node, "holder": c.holder, "idle_secs": idle_secs}),
            );
            actions.push(format!("released unknown idle node {}", c.node));
            continue;
        }

        if is_protected {
            continue;
        }

        if let Some(warn_ts) = c.warned_at {
            if now - warn_ts >= 600 {
                conn.execute("DELETE FROM claims WHERE node = ?1", params![c.node])
                    .map_err(|e| e.to_string())?;
                after_release(conn, &c.node)?;
                log_event(
                    "sweep_release_idle",
                    json!({"node": c.node, "holder": c.holder, "idle_secs": idle_secs}),
                );
                actions.push(format!("released idle node {} from {}", c.node, c.holder));
            }
        } else if idle_secs >= 2700 {
            conn.execute(
                "UPDATE claims SET warned_at = ?1 WHERE node = ?2",
                params![now, c.node],
            )
            .map_err(|e| e.to_string())?;
            log_event(
                "sweep_warn_idle",
                json!({"node": c.node, "holder": c.holder, "idle_secs": idle_secs}),
            );
            actions.push(format!("warned idle node {} held by {}", c.node, c.holder));
        }
    }

    run_queue(conn, now, &mut actions)?;

    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head() -> QueueRow {
        QueueRow {
            node: "ab1001".into(),
            waiter: "tick-b".into(),
            base_prio: 5,
            enqueued_at: 0,
            reason: Some("disk check".into()),
            asked_at: Some(1000),
            asked_holder: Some("tick-a".into()),
            reply: None,
            eta_ts: None,
            replied_at: None,
            escalated_at: None,
            offered_at: None,
        }
    }

    #[test]
    fn silent_holder_escalates_after_the_timeout() {
        let h = head();
        assert!(escalation_due(&h, "tick-a", 1000 + ASK_TIMEOUT_SECS - 1).is_none());
        let q = escalation_due(&h, "tick-a", 1000 + ASK_TIMEOUT_SECS).unwrap();
        assert!(q.contains("tick-b asked tick-a for ab1001 (disk check) 15m ago"), "{}", q);
    }

    #[test]
    fn escalates_once_and_only_for_the_asked_holder() {
        let mut h = head();
        h.escalated_at = Some(5000);
        assert!(escalation_due(&h, "tick-a", 99_999).is_none());
        let h = head();
        assert!(escalation_due(&h, "tick-c", 99_999).is_none());
    }

    #[test]
    fn replies_change_what_escalates() {
        // a "no" is an answer: nothing to escalate
        let mut h = head();
        h.reply = Some("no".into());
        h.replied_at = Some(1100);
        assert!(escalation_due(&h, "tick-a", 99_999).is_none());

        // a promise escalates only once it is overdue by the timeout
        let mut h = head();
        h.reply = Some("yes".into());
        h.eta_ts = Some(3000);
        assert!(escalation_due(&h, "tick-a", 3000 + ASK_TIMEOUT_SECS - 1).is_none());
        let q = escalation_due(&h, "tick-a", 3000 + ASK_TIMEOUT_SECS).unwrap();
        assert!(q.contains("tick-a promised ab1001 to tick-b"), "{}", q);
    }
}
