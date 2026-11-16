use latch_core::{
    ensure_dir, home_dir, is_pid_alive, latchboard_config_dir, latchboard_state_dir,
};
use latch_proto::{ControlRequest, ControlResponse, RunnerState};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{self, Command, Stdio};
use std::time::Duration;

fn find_runner_bin() -> PathBuf {
    if let Ok(b) = env::var("LATCH_RUN_BIN") {
        return PathBuf::from(b);
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("latch-run");
            if sibling.exists() {
                return sibling;
            }
        }
    }
    home_dir().join(".local/bin/latch-run")
}

fn discover_ticks() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let dirs = [
        latchboard_config_dir().join("ticks"),
        home_dir().join(".config/latchd-up/ticks"),
    ];
    for d in dirs {
        if !d.exists() {
            continue;
        }
        if let Ok(entries) = fs::read_dir(&d) {
            let mut paths: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("md"))
                .collect();
            paths.sort();
            for p in paths {
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("tick");
                let id = if let Some((pfx, rest)) = stem.split_once('-') {
                    if pfx.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty() {
                        rest.to_string()
                    } else {
                        stem.to_string()
                    }
                } else {
                    stem.to_string()
                };
                out.push((id, p));
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

fn load_runner_states() -> Vec<RunnerState> {
    let mut states = Vec::new();
    let reg_dir = latchboard_state_dir().join("runners");
    let Ok(entries) = fs::read_dir(reg_dir) else {
        return states;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(raw) = fs::read_to_string(&p) {
            if let Ok(st) = serde_json::from_str::<RunnerState>(&raw) {
                states.push(st);
            }
        }
    }
    states.sort_by(|a, b| a.tick_id.cmp(&b.tick_id));
    states
}

/// What to do with one registry entry / socket pair, given whether the
/// runner PID recorded for it is alive (None: no registry entry) and whether
/// anything is listening on the socket.
fn is_stale(pid_alive: Option<bool>, socket_listening: bool) -> bool {
    match pid_alive {
        Some(alive) => !alive,
        // A socket with no registry entry may belong to a runner that is
        // just starting, so only a dead socket counts.
        None => !socket_listening,
    }
}

/// Remove registry entries and sockets left behind by runners that died
/// without cleaning up. Returns the tick ids that were swept.
fn sweep_stale() -> Vec<String> {
    let state_dir = latchboard_state_dir();
    let reg_dir = state_dir.join("runners");
    let sock_dir = state_dir.join("sockets");
    let mut swept = Vec::new();

    let mut registered = std::collections::HashSet::new();
    for st in load_runner_states() {
        registered.insert(st.tick_id.clone());
        // Checked directly: a dead runner must be found even where
        // is_pid_alive is stubbed out.
        let alive = unsafe { libc::kill(st.pid as i32, 0) == 0 };
        if is_stale(Some(alive), false) {
            let _ = fs::remove_file(reg_dir.join(format!("{}.json", st.tick_id)));
            let _ = fs::remove_file(sock_dir.join(format!("{}.sock", st.tick_id)));
            swept.push(st.tick_id);
        }
    }

    if let Ok(entries) = fs::read_dir(&sock_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("sock") {
                continue;
            }
            let Some(id) = p.file_stem().and_then(|s| s.to_str()).map(String::from) else {
                continue;
            };
            if registered.contains(&id) {
                continue;
            }
            let listening = UnixStream::connect(&p).is_ok();
            if is_stale(None, listening) {
                let _ = fs::remove_file(&p);
                swept.push(id);
            }
        }
    }

    swept.sort();
    swept
}

fn send_control(tick_id: &str, req: &ControlRequest) -> Result<ControlResponse, String> {
    let sock_path = latchboard_state_dir()
        .join("sockets")
        .join(format!("{}.sock", tick_id));
    let mut stream = UnixStream::connect(&sock_path).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    let payload = serde_json::to_string(req).map_err(|e| e.to_string())?;
    stream
        .write_all(payload.as_bytes())
        .map_err(|e| e.to_string())?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut resp_str = String::new();
    stream
        .read_to_string(&mut resp_str)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&resp_str).map_err(|e| e.to_string())
}

fn start_one_tick(tick_id: &str, tick_path: &PathBuf) -> Result<u32, String> {
    let runner_bin = find_runner_bin();
    let log_dir = latchboard_state_dir().join("logs");
    let _ = ensure_dir(&log_dir);
    let log_file = fs::File::create(log_dir.join(format!("{}.log", tick_id)))
        .map_err(|e| e.to_string())?;
    let err_file = log_file.try_clone().map_err(|e| e.to_string())?;

    let child = Command::new(runner_bin)
        .arg(tick_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .map_err(|e| e.to_string())?;

    Ok(child.id())
}

fn stop_one_tick(tick_id: &str) -> Result<String, String> {
    let reg_file = latchboard_state_dir()
        .join("runners")
        .join(format!("{}.json", tick_id));
    let sock_file = latchboard_state_dir()
        .join("sockets")
        .join(format!("{}.sock", tick_id));

    if let Ok(raw) = fs::read_to_string(&reg_file) {
        if let Ok(st) = serde_json::from_str::<RunnerState>(&raw) {
            unsafe {
                libc::kill(st.pid as i32, libc::SIGTERM);
            }
        }
    }
    let _ = fs::remove_file(&reg_file);
    let _ = fs::remove_file(&sock_file);
    Ok(format!("stopped {}", tick_id))
}

fn usage() -> ! {
    eprintln!(
        "Usage: latch-up <command> [tick_id|all|dashboard]
Commands:
  start [tick_id|all]
  stop [tick_id|all]
  restart [tick_id|all|dashboard]
  pause <tick_id|all>
  resume <tick_id|all>
  tick <tick_id>
  sweep
  status [--json]"
    );
    process::exit(2);
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }

    let cmd = args[0].as_str();
    let target = args.get(1).map(|s| s.as_str()).unwrap_or("all");

    match cmd {
        "sweep" => {
            let swept = sweep_stale();
            for id in &swept {
                println!("swept stale runner state for {}", id);
            }
            if swept.is_empty() {
                println!("nothing to sweep");
            }
        }
        "start" => {
            for id in sweep_stale() {
                println!("swept stale runner state for {}", id);
            }
            let ticks = discover_ticks();
            for (id, path) in ticks {
                if target == "all" || target == id {
                    match start_one_tick(&id, &path) {
                        Ok(pid) => println!("started {} (pid {})", id, pid),
                        Err(e) => {
                            eprintln!("failed to start {}: {}", id, e);
                            process::exit(1);
                        }
                    }
                }
            }
        }
        "stop" => {
            let states = load_runner_states();
            for st in states {
                if target == "all" || target == st.tick_id {
                    if let Ok(msg) = stop_one_tick(&st.tick_id) {
                        println!("{}", msg);
                    }
                }
            }
        }
        "restart" => {
            if target == "dashboard" {
                println!("restarted dashboard");
                return;
            }
            let states = load_runner_states();
            for st in states {
                if target == "all" || target == st.tick_id {
                    match send_control(
                        &st.tick_id,
                        &ControlRequest::Restart {
                            reason: "manual".to_string(),
                        },
                    ) {
                        Ok(r) => println!("{}: {}", st.tick_id, r.message),
                        Err(e) => eprintln!("{}: {}", st.tick_id, e),
                    }
                }
            }
        }
        "pause" => {
            for st in load_runner_states() {
                if target == "all" || target == st.tick_id {
                    if let Ok(r) = send_control(&st.tick_id, &ControlRequest::Pause) {
                        println!("{}: {}", st.tick_id, r.message);
                    }
                }
            }
        }
        "resume" => {
            for st in load_runner_states() {
                if target == "all" || target == st.tick_id {
                    if let Ok(r) = send_control(&st.tick_id, &ControlRequest::Resume) {
                        println!("{}: {}", st.tick_id, r.message);
                    }
                }
            }
        }
        "tick" => {
            if target == "all" {
                usage();
            }
            match send_control(target, &ControlRequest::Tick) {
                Ok(r) => println!("{}: {}", target, r.message),
                Err(e) => {
                    eprintln!("{}: {}", target, e);
                    process::exit(1);
                }
            }
        }
        "status" => {
            let as_json = args.iter().any(|a| a == "--json");
            let states = load_runner_states();
            if as_json {
                println!("{}", serde_json::to_string_pretty(&states).unwrap());
            } else {
                println!("RUNNERS ({}):", states.len());
                for st in states {
                    let alive = is_pid_alive(st.pid);
                    let flag = if !alive {
                        "dead"
                    } else if st.paused {
                        "paused"
                    } else if st.needs_you {
                        "needs-you"
                    } else {
                        "running"
                    };
                    println!("  {} [{}] pid={} title={}", st.tick_id, flag, st.pid, st.title);
                }
            }
        }
        _ => usage(),
    }
}

#[cfg(test)]
mod tests {
    use super::is_stale;

    #[test]
    fn stale_rules() {
        // registered runner: only its PID matters
        assert!(is_stale(Some(false), true));
        assert!(!is_stale(Some(true), false));
        // unregistered socket: stale only when nothing listens
        assert!(is_stale(None, false));
        assert!(!is_stale(None, true));
    }
}
