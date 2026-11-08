mod daemon;
mod db;
mod directives;

use latch_core::{latchboard_config_dir, latchd_config_dir, now_epoch};
use rusqlite::params;
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
  claim <node> --holder <id> [-p <1-9>]
  release <node> [--holder <id>]
  wait <node> --waiter <id> [-p <1-9>]
  grant-take <node> --session <id>
  take <node> --session <id>
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
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
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
                    match db::claim_node(&conn, node, &holder, eff_prio, is_known) {
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
            let mut prio: u8 = 5;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--waiter" | "--holder" | "-w" if i + 1 < args.len() => {
                        waiter = args[i + 1].clone();
                        i += 2;
                    }
                    "-p" | "--prio" if i + 1 < args.len() => {
                        prio = args[i + 1].parse().unwrap_or(5).clamp(1, 9);
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
            match db::enqueue_wait(&conn, node, &waiter, prio) {
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
            let mut stmt = conn
                .prepare("SELECT node, holder, prio, claimed_at, last_used_at, warned_at, known_node FROM claims ORDER BY node")
                .unwrap();
            let claims: Vec<db::ClaimRow> = stmt
                .query_map(params![], |r| {
                    Ok(db::ClaimRow {
                        node: r.get(0)?,
                        holder: r.get(1)?,
                        prio: r.get(2)?,
                        claimed_at: r.get(3)?,
                        last_used_at: r.get(4)?,
                        warned_at: r.get(5)?,
                        known_node: r.get::<_, i64>(6)? != 0,
                    })
                })
                .unwrap()
                .filter_map(Result::ok)
                .collect();

            let mut qstmt = conn
                .prepare("SELECT node, waiter, base_prio, enqueued_at FROM queue ORDER BY node, enqueued_at")
                .unwrap();
            let queue: Vec<serde_json::Value> = qstmt
                .query_map(params![], |r| {
                    let row = db::QueueRow {
                        node: r.get(0)?,
                        waiter: r.get(1)?,
                        base_prio: r.get(2)?,
                        enqueued_at: r.get(3)?,
                    };
                    let eff = row.effective_prio(now);
                    Ok(json!({
                        "node": row.node,
                        "waiter": row.waiter,
                        "base_prio": row.base_prio,
                        "effective_prio": eff,
                        "enqueued_at": row.enqueued_at,
                    }))
                })
                .unwrap()
                .filter_map(Result::ok)
                .collect();

            if as_json {
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
                    println!("  {} held by {} (p{})", c.node, c.holder, c.prio);
                }
                println!("QUEUE ({}):", queue.len());
                for q in &queue {
                    println!(
                        "  {} waiting by {} (p{})",
                        q["node"].as_str().unwrap_or(""),
                        q["waiter"].as_str().unwrap_or(""),
                        q["effective_prio"].as_u64().unwrap_or(0)
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
