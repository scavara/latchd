use latch_core::{handover_state_dir, now_epoch};
use latch_proto::{Directive, DirectiveMode};
use std::fs;
use std::path::PathBuf;

pub fn directives_path() -> PathBuf {
    handover_state_dir().join("directives.json")
}

pub fn load_directives() -> Vec<Directive> {
    let path = directives_path();
    let Ok(data) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

/// Split raw directive records into those to keep and the ids of `prio_floor`
/// directives whose `until_ts` has passed. Other modes are never dropped, and
/// records are kept as raw JSON so fields this build does not know survive.
fn split_expired_floors(
    records: Vec<serde_json::Value>,
    now: i64,
) -> (Vec<serde_json::Value>, Vec<u64>) {
    let mut kept = Vec::new();
    let mut expired = Vec::new();
    for rec in records {
        let is_floor = rec["mode"].as_str() == Some("prio_floor");
        let until = rec["until_ts"].as_i64();
        if is_floor && until.is_some_and(|u| u <= now) {
            expired.push(rec["id"].as_u64().unwrap_or(0));
        } else {
            kept.push(rec);
        }
    }
    (kept, expired)
}

/// Remove expired `prio_floor` directives from `directives.json`. Returns the
/// ids removed. A missing or malformed file is left untouched.
pub fn prune_expired_prio_floors() -> Vec<u64> {
    let path = directives_path();
    let Ok(data) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(records) = serde_json::from_str::<Vec<serde_json::Value>>(&data) else {
        return Vec::new();
    };
    let (kept, expired) = split_expired_floors(records, now_epoch());
    if expired.is_empty() {
        return expired;
    }
    let Ok(mut content) = serde_json::to_string_pretty(&kept) else {
        return Vec::new();
    };
    content.push('\n');
    // Write-then-rename so a concurrent reader never sees a partial file.
    let tmp = path.with_extension("tmp-latchd");
    if fs::write(&tmp, content).is_err() || fs::rename(&tmp, &path).is_err() {
        let _ = fs::remove_file(&tmp);
        return Vec::new();
    }
    expired
}

pub fn node_matches_pattern(node: &str, pattern: &str) -> bool {
    if pattern == "*" || pattern == "all" || pattern == node {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return node.starts_with(prefix);
    }
    false
}

pub fn check_node_directives(
    node: &str,
    holder: &str,
    base_prio: u8,
) -> Result<(u8, Vec<String>), String> {
    let now = now_epoch();
    let directives = load_directives();
    let mut effective_prio = base_prio;
    let mut warnings = Vec::new();

    for d in directives {
        if d.until_ts <= now {
            continue;
        }
        if let Some(from) = d.from_ts {
            if now < from {
                continue;
            }
        }
        if !d.nodes.iter().any(|pat| node_matches_pattern(node, pat)) {
            continue;
        }
        match d.mode {
            DirectiveMode::Block => {
                return Err(format!("directive #{}: {}", d.id, d.text));
            }
            DirectiveMode::Owner => {
                if let Some(ref req_owner) = d.owner {
                    if req_owner != holder {
                        return Err(format!(
                            "directive #{}: reserved for {} ({})",
                            d.id, req_owner, d.text
                        ));
                    }
                }
            }
            DirectiveMode::MinPrio => {
                if let Some(min_p) = d.prio {
                    if effective_prio < min_p {
                        return Err(format!(
                            "directive #{}: requires priority >= p{} ({})",
                            d.id, min_p, d.text
                        ));
                    }
                }
            }
            DirectiveMode::PrioFloor => {
                if let Some(floor) = d.prio {
                    if effective_prio < floor {
                        effective_prio = floor;
                    }
                }
            }
            DirectiveMode::Warn => {
                warnings.push(format!("directive #{}: {}", d.id, d.text));
            }
            DirectiveMode::Pause => {
                if let Some(ref target) = d.pause_target {
                    if target == "all" || target == holder {
                        return Err(format!("directive #{}: tick {} paused", d.id, holder));
                    }
                }
            }
        }
    }

    Ok((effective_prio, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_expired_floors_are_dropped() {
        let records = vec![
            json!({"id": 1, "mode": "prio_floor", "until_ts": 100, "extra": "kept"}),
            json!({"id": 2, "mode": "prio_floor", "until_ts": 50}),
            json!({"id": 3, "mode": "block", "until_ts": 50}),
            json!({"id": 4, "mode": "prio_floor"}),
            json!({"id": 5, "mode": "prio_floor", "until_ts": 60}),
        ];
        let (kept, expired) = split_expired_floors(records, 60);
        assert_eq!(expired, vec![2, 5]);
        let ids: Vec<u64> = kept.iter().map(|r| r["id"].as_u64().unwrap()).collect();
        assert_eq!(ids, vec![1, 3, 4]);
        assert_eq!(kept[0]["extra"], "kept");
    }
}
