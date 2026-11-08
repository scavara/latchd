use crate::db::{heartbeat_protected_nodes, log_event, ClaimRow, QueueRow};
use latch_core::now_epoch;
use rusqlite::{params, Connection};
use serde_json::json;

pub fn run_sweep(conn: &Connection) -> Result<Vec<String>, String> {
    let now = now_epoch();
    let hb_protected = heartbeat_protected_nodes();
    let mut actions = Vec::new();

    let mut stmt = conn
        .prepare("SELECT node, holder, prio, claimed_at, last_used_at, warned_at, known_node FROM claims")
        .map_err(|e| e.to_string())?;

    let claims: Vec<ClaimRow> = stmt
        .query_map([], |r| {
            Ok(ClaimRow {
                node: r.get(0)?,
                holder: r.get(1)?,
                prio: r.get(2)?,
                claimed_at: r.get(3)?,
                last_used_at: r.get(4)?,
                warned_at: r.get(5)?,
                known_node: r.get::<_, i64>(6)? != 0,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    for c in claims {
        let idle_secs = now - c.last_used_at;
        let is_protected = hb_protected.contains(&(c.holder.clone(), c.node.clone()));

        if !c.known_node && idle_secs >= 600 && !is_protected {
            conn.execute("DELETE FROM claims WHERE node = ?1", params![c.node])
                .map_err(|e| e.to_string())?;
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

    let mut q_stmt = conn
        .prepare("SELECT node, waiter, base_prio, enqueued_at FROM queue")
        .map_err(|e| e.to_string())?;
    let queue_rows: Vec<QueueRow> = q_stmt
        .query_map([], |r| {
            Ok(QueueRow {
                node: r.get(0)?,
                waiter: r.get(1)?,
                base_prio: r.get(2)?,
                enqueued_at: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    for q in queue_rows {
        let eff_prio = q.effective_prio(now);
        if let Ok((holder, holder_prio)) = conn.query_row(
            "SELECT holder, prio FROM claims WHERE node = ?1",
            params![q.node],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, u8>(1)?)),
        ) {
            if eff_prio >= holder_prio.saturating_add(2) {
                log_event(
                    "yield_request",
                    json!({
                        "node": q.node,
                        "waiter": q.waiter,
                        "effective_prio": eff_prio,
                        "holder": holder,
                        "holder_prio": holder_prio
                    }),
                );
                actions.push(format!(
                    "yield requested on {}: {} (p{}) vs {} (p{})",
                    q.node, q.waiter, eff_prio, holder, holder_prio
                ));
            }
        }
    }

    Ok(actions)
}
