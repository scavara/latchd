mod daemon;
mod db;
mod directives;

use latch_core::{latchboard_config_dir, latchd_config_dir, now_epoch};
use serde_json::json;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::process;
use std::thread;
use std::time::Duration;

fn load_known_nodes() -> BTreeSet<String> {
    let mut nodes = BTreeSet::new();
    for dir in [latchd_config_dir(), latchboard_config_dir()] {
        let inv = dir.join("nodes");
        if let Ok(content) = fs::read_to_string(inv) {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('[') {
                    continue;
                }
                if let Some(first) = trimmed.split_whitespace().next() {
                    if !first.contains('=') {
                        nodes.insert(first.to_string());
                    }
                }
            }
        }
    }
    nodes
}

fn usage() -> ! {
    eprintln!(
        "Usage: latchd <command> [args]
Commands:
  claim <node> --holder <id> [-p <1-9>] [--incident]
  release <node> [--holder <id>]
  wait <node> --waiter <id> [-p <1-9>] [--reason <text>]
      join the queue (first come, first served) and ask the holder for the node
  reply <node> yes|no [--eta <30m>] [--holder <id>]
      answer the first waiter: `yes` hands over now, `yes --eta` promises it
  grant-take <node> --session <id>
  take <node> --session <id>
      incident ticks only: take a node without asking
  sweep
  nodes [glob]
  status [--json]
  daemon"
    );
    process::exit(2);
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }

    let conn = match db::open_db() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("db error: {}", e);
            process::exit(1);
        }
    };

    match args[0].as_str() {
        "claim" => {
            if args.len() < 2 {
                usage();
            }
            let node = &args[1];
            let mut holder = env::var("LATCH_SESSION").unwrap_or_else(|_| "default".to_string());
            let mut prio: u8 = 5;
            let mut incident = env::var("LATCH_ROLE").as_deref() == Ok("incident");
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--incident" => {
                        incident = true;
                        i += 1;
                    }
                    "--holder" | "-h" if i + 1 < args.len() => {
                        holder = args[i + 1].clone();
                        i += 2;
                    }
                    "-p" | "--prio" if i + 1 < args.len() => {
                        prio = args[i + 1].parse().unwrap_or(5).clamp(1, 9);
                        i += 2;
                    }
                    _ => i += 1,
                }
            }

            match directives::check_node_directives(node, &holder, prio) {
                Ok((eff_prio, warnings)) => {
                    for w in warnings {
                        eprintln!("WARN: {}", w);
                    }
                    let known = load_known_nodes();
                    let is_known = known.is_empty() || known.contains(node);
                    match db::claim_node(&conn, node, &holder, eff_prio, is_known, incident) {
                        Ok(msg) => println!("{}", msg),
                        Err(e) => {
                            eprintln!("{}", e);
                            process::exit(1);
                        }
                    }
                }
                Err(block_reason) => {
                    eprintln!("refused: {}", block_reason);
                    process::exit(1);
                }
            }
        }
        "release" => {
            if args.len() < 2 {
                usage();
            }
            let node = &args[1];
            let mut holder: Option<String> = None;
            let mut i = 2;
            while i < args.len() {
                if (args[i] == "--holder" || args[i] == "-h") && i + 1 < args.len() {
                    holder = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    i += 1;
                }
            }
            match db::release_node(&conn, node, holder.as_deref()) {
                Ok(msg) => println!("{}", msg),
                Err(e) => {
                    eprintln!("{}", e);
                    process::exit(1);
                }
            }
        }
        "wait" => {
            if args.len() < 2 {
                usage();
            }
            let node = &args[1];
            let mut waiter = env::var("LATCH_SESSION").unwrap_or_else(|_| "default".to_string());
            let mut prio: Option<u8> = None;
            let mut reason: Option<String> = None;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--reason" if i + 1 < args.len() => {
                        reason = Some(args[i + 1].clone());
                        i += 2;
                    }
                    "--waiter" | "--holder" | "-w" if i + 1 < args.len() => {
                        waiter = args[i + 1].clone();
                        i += 2;
                    }
                    "-p" | "--prio" if i + 1 < args.len() => {
                        prio = Some(args[i + 1].parse().unwrap_or(5).clamp(1, 9));
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
            match db::enqueue_wait(&conn, node, &waiter, prio, reason.as_deref()) {
                Ok(msg) => println!("{}", msg),
                Err(e) => {
                    eprintln!("{}", e);
                    process::exit(1);
                }
            }
        }
        "reply" => {
            if args.len() < 3 {
                usage();
            }
            let node = &args[1];
            let yes = match args[2].as_str() {
                "yes" | "y" => true,
                "no" | "n" => false,
                _ => usage(),
            };
            let mut holder = env::var("LATCH_SESSION").unwrap_or_else(|_| "default".to_string());
            let mut eta_secs: Option<i64> = None;
            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--holder" | "-h" if i + 1 < args.len() => {
                        holder = args[i + 1].clone();
                        i += 2;
                    }
                    "--eta" if i + 1 < args.len() => {
                        match latch_proto::parse_duration_secs(&args[i + 1]) {
                            Some(secs) => eta_secs = Some(secs as i64),
                            None => {
                                eprintln!("invalid --eta: {} (use e.g. 20m, 2h)", args[i + 1]);
                                process::exit(2);
                            }
                        }
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
            match db::reply_takeover(&conn, node, &holder, yes, eta_secs) {
                Ok(msg) => println!("{}", msg),
                Err(e) => {
                    eprintln!("{}", e);
                    process::exit(1);
                }
            }
        }
        "grant-take" => {
            if args.len() < 2 {
                usage();
            }
            let node = &args[1];
            let mut session = env::var("LATCH_SESSION").unwrap_or_else(|_| "incident".to_string());
            let mut i = 2;
            while i < args.len() {
                if args[i] == "--session" && i + 1 < args.len() {
                    session = args[i + 1].clone();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if let Err(e) = db::record_take_grant(&conn, node, &session) {
                eprintln!("{}", e);
                process::exit(1);
            }
            println!("recorded take grant for {} ({})", node, session);
        }
        "take" => {
            if args.len() < 2 {
                usage();
            }
            let node = &args[1];
            let mut session = env::var("LATCH_SESSION").unwrap_or_else(|_| "incident".to_string());
            let mut i = 2;
            while i < args.len() {
                if args[i] == "--session" && i + 1 < args.len() {
                    session = args[i + 1].clone();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            match db::take_node_incident(&conn, node, &session) {
                Ok(msg) => println!("{}", msg),
                Err(e) => {
                    eprintln!("{}", e);
                    process::exit(1);
                }
            }
        }
        "sweep" => match daemon::run_sweep(&conn) {
            Ok(actions) => {
                for a in actions {
                    println!("{}", a);
                }
            }
            Err(e) => {
                eprintln!("{}", e);
                process::exit(1);
            }
        },
        "nodes" => {
            let pat = args.get(1).map(|s| s.as_str()).unwrap_or("*");
            for n in load_known_nodes() {
                if directives::node_matches_pattern(&n, pat) {
                    println!("{}", n);
                }
            }
        }
        "status" => {
            let as_json = args.iter().any(|a| a == "--json");
            let now = now_epoch();
            let claims = db::load_claims(&conn).unwrap_or_default();
            let queue_rows = db::load_queue(&conn).unwrap_or_default();

            // Position in line restarts at 1 for each node.
            let mut positions: Vec<usize> = Vec::with_capacity(queue_rows.len());
            for (i, q) in queue_rows.iter().enumerate() {
                let same_node = i > 0 && queue_rows[i - 1].node == q.node;
                positions.push(if same_node { positions[i - 1] + 1 } else { 1 });
            }

            if as_json {
                let queue: Vec<serde_json::Value> = queue_rows
                    .iter()
                    .zip(&positions)
                    .map(|(q, pos)| {
                        let mut v = serde_json::to_value(q).unwrap_or_else(|_| json!({}));
                        v["position"] = json!(pos);
                        v
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "claims": claims,
                        "queue": queue,
                    }))
                    .unwrap()
                );
            } else {
                println!("CLAIMS ({}):", claims.len());
                for c in &claims {
                    let tag = if c.incident { " [incident: off-limits]" } else { "" };
                    println!("  {} held by {} (p{}){}", c.node, c.holder, c.prio, tag);
                }
                println!("QUEUE ({}):", queue_rows.len());
                for (q, pos) in queue_rows.iter().zip(&positions) {
                    let state = match (q.reply.as_deref(), q.asked_holder.as_deref(), q.eta_ts) {
                        (Some("yes"), Some(h), Some(eta)) => {
                            format!(", {} promised it in {}", h, db::fmt_span(eta - now))
                        }
                        (Some("no"), Some(h), _) => format!(", {} declined", h),
                        (None, Some(h), _) if q.escalated_at.is_some() => {
                            format!(", {} silent: operator asked", h)
                        }
                        (None, Some(h), _) => format!(
                            ", asked {} {} ago",
                            h,
                            db::fmt_span(now - q.asked_at.unwrap_or(now))
                        ),
                        _ if q.offered_at.is_some() => ", node is free: offered".to_string(),
                        _ => String::new(),
                    };
                    println!(
                        "  {} waiting by {} ({} in line, {}{})",
                        q.node,
                        q.waiter,
                        pos,
                        db::fmt_span(now - q.enqueued_at),
                        state
                    );
                }
            }
        }
        "daemon" => loop {
            let _ = daemon::run_sweep(&conn);
            thread::sleep(Duration::from_secs(15));
        },
        _ => usage(),
    }
}
