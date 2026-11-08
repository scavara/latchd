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

pub fn save_directives(list: &[Directive]) -> std::io::Result<()> {
    let path = directives_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(list)?;
    fs::write(path, content)
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
                        return Err(format!("directive #{}: loop {} paused", d.id, holder));
                    }
                }
            }
        }
    }

    Ok((effective_prio, warnings))
}
