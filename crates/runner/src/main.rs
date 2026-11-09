use latch_core::{
    append_jsonl, ensure_dir, handover_state_dir, home_dir, latchboard_config_dir,
    latchboard_state_dir, now_epoch,
};
use latch_proto::{parse_duration_secs, ControlRequest, ControlResponse, RunnerState};
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::io::{FromRawFd, RawFd};
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone)]
struct LoopSpec {
    loop_id: String,
    path: PathBuf,
    title: String,
    every_secs: Option<u64>,
    role: Option<String>,
    runner_cmd: String,
    restart_on: Vec<String>,
    agent_args: Vec<String>,
    prompt: String,
}

fn file_fingerprint(path: &Path) -> u64 {
    let Ok(meta) = fs::metadata(path) else {
        return 0;
    };
    let len = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    len.wrapping_mul(31).wrapping_add(mtime)
}

fn find_loop_file(loop_arg: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(loop_arg);
    if direct.exists() {
        return Some(direct);
    }
    let dirs = [
        latchboard_config_dir().join("loops"),
        home_dir().join(".config/morning-startup/loops"),
    ];
    for d in dirs {
        if !d.exists() {
            continue;
        }
        let exact = d.join(format!("{}.md", loop_arg));
        if exact.exists() {
            return Some(exact);
        }
        if let Ok(entries) = fs::read_dir(&d) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if stem == loop_arg || stem.ends_with(&format!("-{}", loop_arg)) {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn derive_loop_id(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("default");
    if let Some((prefix, rest)) = stem.split_once('-') {
        if prefix.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty() {
            return rest.to_string();
        }
    }
    stem.to_string()
}

fn parse_loop_file(path: &Path) -> Result<LoopSpec, String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let loop_id = derive_loop_id(path);
    let mut headers: HashMap<String, String> = HashMap::new();
    let mut prompt_lines = Vec::new();
    let mut in_prompt = false;

    for line in raw.lines() {
        if !in_prompt && line.trim() == "---" {
            in_prompt = true;
            continue;
        }
        if !in_prompt {
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_lowercase(), v.trim().to_string());
            }
        } else {
            prompt_lines.push(line);
        }
    }

    let title = headers
        .get("title")
        .cloned()
        .unwrap_or_else(|| loop_id.clone());
    let every_secs = headers.get("every").and_then(|v| parse_duration_secs(v));
    let role = headers.get("role").cloned();
    let runner_cmd = env::var("LATCH_AGENT_BIN")
        .ok()
        .or_else(|| headers.get("runner").cloned())
        .unwrap_or_else(|| "claude".to_string());

    let restart_on = headers
        .get("restart_on")
        .map(|s| {
            s.split(|c: char| c == ',' || c.is_whitespace())
                .filter(|x| !x.is_empty())
                .map(|x| x.to_string())
                .collect()
        })
        .unwrap_or_else(|| {
            vec![
                "crash".to_string(),
                "upgrade".to_string(),
                "change".to_string(),
            ]
        });

    let args_raw = headers
        .get("agent_args")
        .or_else(|| headers.get("claude_args"))
        .cloned()
        .unwrap_or_default();
    let agent_args = args_raw
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();

    Ok(LoopSpec {
        loop_id,
        path: path.to_path_buf(),
        title,
        every_secs,
        role,
        runner_cmd,
        restart_on,
        agent_args,
        prompt: prompt_lines.join("\n").trim().to_string(),
    })
}

fn common_md_path() -> PathBuf {
    let p1 = latchboard_config_dir().join("common.md");
    if p1.exists() {
        return p1;
    }
    home_dir().join(".config/morning-startup/common.md")
}

fn spawn_session_pty(spec: &LoopSpec) -> Result<(Child, FilePair), String> {
    let mut master_fd: RawFd = -1;
    let mut slave_fd: RawFd = -1;
    let pty_ok = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ) == 0
    };

    let prio = if spec.role.as_deref() == Some("incident") {
        "9"
    } else {
        "5"
    };

    if pty_ok {
        let slave_in = unsafe { Stdio::from_raw_fd( libc::dup(slave_fd) ) };
        let slave_out = unsafe { Stdio::from_raw_fd( libc::dup(slave_fd) ) };
        let slave_err = unsafe { Stdio::from_raw_fd(slave_fd) };

        let mut cmd = Command::new(&spec.runner_cmd);
        cmd.args(&spec.agent_args)
            .env("LATCH_SESSION", &spec.loop_id)
            .env("LATCH_TITLE", &spec.title)
            .env("LATCH_PRIO", prio)
            .stdin(slave_in)
            .stdout(slave_out)
            .stderr(slave_err);

        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }

        let child = cmd.spawn().map_err(|e| e.to_string())?;
        let writer = unsafe { fs::File::from_raw_fd( libc::dup(master_fd) ) };
        let reader = unsafe { fs::File::from_raw_fd(master_fd) };
        Ok((child, FilePair { reader, writer }))
    } else {
        let mut cmd = Command::new(&spec.runner_cmd);
        let mut child = cmd
            .args(&spec.agent_args)
            .env("LATCH_SESSION", &spec.loop_id)
            .env("LATCH_TITLE", &spec.title)
            .env("LATCH_PRIO", prio)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;

        let stdin = child.stdin.take().ok_or("missing stdin")?;
        let stdout = child.stdout.take().ok_or("missing stdout")?;
        let writer_fd = std::os::unix::io::IntoRawFd::into_raw_fd(stdin);
        let reader_fd = std::os::unix::io::IntoRawFd::into_raw_fd(stdout);
        let writer = unsafe { fs::File::from_raw_fd(writer_fd) };
        let reader = unsafe { fs::File::from_raw_fd(reader_fd) };
        Ok((child, FilePair { reader, writer }))
    }
}

struct FilePair {
    reader: fs::File,
    writer: fs::File,
}

fn save_registry(state: &RunnerState) {
    let reg_dir = latchboard_state_dir().join("runners");
    let _ = ensure_dir(&reg_dir);
    let reg_file = reg_dir.join(format!("{}.json", state.loop_id));
    if let Ok(txt) = serde_json::to_string_pretty(state) {
        let _ = fs::write(reg_file, txt);
    }
}

fn record_handover_note(loop_id: &str, reason: &str) {
    let inbox_dir = handover_state_dir().join("handover").join(loop_id);
    let _ = ensure_dir(&inbox_dir);
    let ts = now_epoch();
    let payload = json!({
        "ts": ts,
        "from": loop_id,
        "to": loop_id,
        "nodes": [],
        "note": format!("auto-handover before restart ({})", reason),
    });
    let file = inbox_dir.join(format!("{}-runner-handover.json", ts));
    if let Ok(txt) = serde_json::to_string_pretty(&payload) {
        let _ = fs::write(file, txt);
    }
    let ev_path = handover_state_dir().join("events.jsonl");
    let _ = append_jsonl(
        &ev_path,
        &json!({
            "ts": ts,
            "event": "runner_restart",
            "details": {"loop_id": loop_id, "reason": reason}
        }),
    );
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("Usage: latch-run <loop-id-or-path> [--once-init]");
        process::exit(2);
    }

    let loop_arg = &args[0];
    let Some(loop_path) = find_loop_file(loop_arg) else {
        eprintln!("loop file not found for: {}", loop_arg);
        process::exit(1);
    };

    let mut spec = match parse_loop_file(&loop_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to parse loop file: {}", e);
            process::exit(1);
        }
    };

    let state_dir = latchboard_state_dir();
    let sock_dir = state_dir.join("sockets");
    let _ = ensure_dir(&sock_dir);
    let sock_path = sock_dir.join(format!("{}.sock", spec.loop_id));
    let _ = fs::remove_file(&sock_path);

    let listener = match UnixListener::bind(&sock_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("failed to bind socket {:?}: {}", sock_path, e);
            process::exit(1);
        }
    };
    let _ = listener.set_nonblocking(true);

    let now = now_epoch();
    let initial_state = RunnerState {
        loop_id: spec.loop_id.clone(),
        title: spec.title.clone(),
        pid: process::id(),
        role: spec.role.clone(),
        every_secs: spec.every_secs,
        paused: false,
        needs_you: false,
        last_tick_ts: now,
        next_tick_ts: spec.every_secs.map(|s| now + s as i64),
        socket_path: sock_path.to_string_lossy().to_string(),
    };
    save_registry(&initial_state);

    let shared_state = Arc::new(Mutex::new(initial_state));
    let pending_ticks = Arc::new(Mutex::new(0u32));
    let pending_restart: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let exe_path = env::current_exe().unwrap_or_else(|_| PathBuf::from(""));
    let mut loop_fp = file_fingerprint(&spec.path);
    let mut common_fp = file_fingerprint(&common_md_path());
    let mut bin_fp = file_fingerprint(&exe_path);

    let (mut child, io_pair) = match spawn_session_pty(&spec) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("failed to spawn runner command '{}': {}", spec.runner_cmd, e);
            process::exit(1);
        }
    };

    let writer_arc = Arc::new(Mutex::new(io_pair.writer));
    if !spec.prompt.is_empty() {
        if let Ok(mut w) = writer_arc.lock() {
            let _ = writeln!(w, "{}", spec.prompt);
        }
    }

    let state_for_reader = Arc::clone(&shared_state);
    let mut reader = io_pair.reader;
    thread::spawn(move || {
        let mut buf = [0u8; 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]);
                    if text.contains("NEEDS_YOU")
                        || text.contains("[needs-you]")
                        || text.contains("waiting for user")
                    {
                        if let Ok(mut st) = state_for_reader.lock() {
                            st.needs_you = true;
                            save_registry(&st);
                        }
                    }
                }
                Err(_) => break,
            }
        }
    });

    loop {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut req_buf = String::new();
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let _ = stream.read_to_string(&mut req_buf);
            if let Ok(req) = serde_json::from_str::<ControlRequest>(req_buf.trim()) {
                let resp = match req {
                    ControlRequest::Status => {
                        let st = shared_state.lock().unwrap().clone();
                        ControlResponse {
                            ok: true,
                            message: "ok".to_string(),
                            state: Some(st),
                        }
                    }
                    ControlRequest::Pause => {
                        let mut st = shared_state.lock().unwrap();
                        st.paused = true;
                        save_registry(&st);
                        ControlResponse {
                            ok: true,
                            message: "paused".to_string(),
                            state: Some(st.clone()),
                        }
                    }
                    ControlRequest::Resume => {
                        let mut st = shared_state.lock().unwrap();
                        st.paused = false;
                        st.needs_you = false;
                        save_registry(&st);
                        ControlResponse {
                            ok: true,
                            message: "resumed".to_string(),
                            state: Some(st.clone()),
                        }
                    }
                    ControlRequest::Tick => {
                        *pending_ticks.lock().unwrap() += 1;
                        let st = shared_state.lock().unwrap().clone();
                        ControlResponse {
                            ok: true,
                            message: "tick scheduled".to_string(),
                            state: Some(st),
                        }
                    }
                    ControlRequest::Restart { reason } => {
                        *pending_restart.lock().unwrap() = Some(reason.clone());
                        let st = shared_state.lock().unwrap().clone();
                        ControlResponse {
                            ok: true,
                            message: format!("restart queued ({})", reason),
                            state: Some(st),
                        }
                    }
                };
                if let Ok(out) = serde_json::to_string(&resp) {
                    let _ = stream.write_all(out.as_bytes());
                }
            }
        }

        let now_ts = now_epoch();
        {
            let mut st = shared_state.lock().unwrap();
            if !st.paused {
                if let Some(next_ts) = st.next_tick_ts {
                    if now_ts >= next_ts {
                        *pending_ticks.lock().unwrap() += 1;
                        st.last_tick_ts = now_ts;
                        st.next_tick_ts = st.every_secs.map(|s| now_ts + s as i64);
                        save_registry(&st);
                    }
                }
            }
        }

        let ticks_to_send = {
            let mut pt = pending_ticks.lock().unwrap();
            let count = *pt;
            *pt = 0;
            count
        };
        if ticks_to_send > 0 {
            if let Ok(mut w) = writer_arc.lock() {
                for _ in 0..ticks_to_send {
                    let _ = writeln!(w, "TICK {}", now_ts);
                }
            }
            let mut st = shared_state.lock().unwrap();
            st.last_tick_ts = now_ts;
            save_registry(&st);
        }

        let new_loop_fp = file_fingerprint(&spec.path);
        let new_common_fp = file_fingerprint(&common_md_path());
        let new_bin_fp = file_fingerprint(&exe_path);

        if (new_loop_fp != loop_fp || new_common_fp != common_fp)
            && spec.restart_on.iter().any(|r| r == "change")
        {
            loop_fp = new_loop_fp;
            common_fp = new_common_fp;
            *pending_restart.lock().unwrap() = Some("change".to_string());
        } else if new_bin_fp != bin_fp && spec.restart_on.iter().any(|r| r == "upgrade") {
            bin_fp = new_bin_fp;
            *pending_restart.lock().unwrap() = Some("upgrade".to_string());
        }

        if let Ok(Some(_status)) = child.try_wait() {
            if spec.restart_on.iter().any(|r| r == "crash") {
                *pending_restart.lock().unwrap() = Some("crash".to_string());
            } else {
                break;
            }
        }

        let restart_reason = pending_restart.lock().unwrap().take();
        if let Some(reason) = restart_reason {
            record_handover_note(&spec.loop_id, &reason);
            let _ = child.kill();
            let _ = child.wait();
            if let Ok(updated) = parse_loop_file(&spec.path) {
                spec = updated;
            }
            if let Ok((new_child, new_io)) = spawn_session_pty(&spec) {
                child = new_child;
                if let Ok(mut w) = writer_arc.lock() {
                    *w = new_io.writer;
                    if !spec.prompt.is_empty() {
                        let _ = writeln!(w, "{}", spec.prompt);
                    }
                }
                let mut st = shared_state.lock().unwrap();
                st.title = spec.title.clone();
                st.every_secs = spec.every_secs;
                st.needs_you = false;
                save_registry(&st);
            }
        }

        thread::sleep(Duration::from_millis(100));
    }

    let _ = fs::remove_file(&sock_path);
}
