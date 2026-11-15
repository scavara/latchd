use latch_core::{
    append_jsonl, config_file_path, config_flag, ensure_dir, handover_state_dir, home_dir,
    latchboard_config_dir, latchboard_state_dir, load_config, now_epoch,
};
use latch_proto::{parse_duration_secs, ControlRequest, ControlResponse, RunnerState};
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// Set by SIGTERM/SIGINT/SIGHUP so the main thread can stop the agent and
/// clean up before exiting.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn on_shutdown_signal(_sig: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// Millisecond timestamp of the agent's most recent output (0: none yet since
/// it was spawned).
static LAST_OUTPUT_MS: AtomicI64 = AtomicI64::new(0);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// How long a freshly started agent gets to switch its terminal to raw mode
/// (i.e. bring up its TUI) before the prompt is sent line-style anyway.
/// `LATCH_PROMPT_WAIT_MS` overrides it, for agents that read plain lines.
const PROMPT_WAIT: Duration = Duration::from_secs(10);

fn line_prompt_wait() -> Duration {
    env::var("LATCH_PROMPT_WAIT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(PROMPT_WAIT)
}
/// A TUI gets the prompt once it has stopped drawing for this long...
const TUI_QUIET_MS: i64 = 1000;
/// ...or after this long regardless.
const TUI_WAIT: Duration = Duration::from_secs(15);

/// Decide whether a held-back prompt can go to the agent now. `raw` is whether
/// the agent runs a TUI, `waited` the time since it was spawned, `quiet_ms`
/// the time since it last wrote output (None: it has not written any yet).
fn prompt_ready(raw: bool, waited: Duration, quiet_ms: Option<i64>, line_wait: Duration) -> bool {
    if raw {
        quiet_ms.is_some_and(|q| q >= TUI_QUIET_MS) || waited >= TUI_WAIT
    } else {
        waited >= line_wait
    }
}

#[derive(Debug, Clone)]
struct TickSpec {
    tick_id: String,
    path: PathBuf,
    title: String,
    every_secs: Option<u64>,
    role: Option<String>,
    runner_cmd: String,
    restart_on: Vec<String>,
    agent_args: Vec<String>,
    fullscreen: bool,
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

fn find_tick_file(tick_arg: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(tick_arg);
    if direct.exists() {
        return Some(direct);
    }
    let dirs = [
        latchboard_config_dir().join("ticks"),
        home_dir().join(".config/latchd-up/ticks"),
    ];
    for d in dirs {
        if !d.exists() {
            continue;
        }
        let exact = d.join(format!("{}.md", tick_arg));
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
                if stem == tick_arg || stem.ends_with(&format!("-{}", tick_arg)) {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn derive_tick_id(path: &Path) -> String {
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

fn parse_tick_file(path: &Path) -> Result<TickSpec, String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let tick_id = derive_tick_id(path);
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
        .unwrap_or_else(|| tick_id.clone());
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
    let mut agent_args: Vec<String> = args_raw
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    let fullscreen = apply_agent_config(&load_config(), &mut agent_args);

    Ok(TickSpec {
        tick_id,
        path: path.to_path_buf(),
        title,
        every_secs,
        role,
        runner_cmd,
        restart_on,
        agent_args,
        fullscreen,
        prompt: prompt_lines.join("\n").trim().to_string(),
    })
}

/// Fold the config file's agent settings into the tick's arguments. A flag the
/// tick already sets in `agent_args` wins over the config value. Returns
/// whether the agent TUI should start fullscreen.
fn apply_agent_config(cfg: &HashMap<String, String>, agent_args: &mut Vec<String>) -> bool {
    for (key, flag) in [("agent_model", "--model"), ("agent_effort", "--effort")] {
        let Some(value) = cfg.get(key).filter(|v| !v.is_empty()) else {
            continue;
        };
        let prefix = format!("{}=", flag);
        if agent_args.iter().any(|a| a == flag || a.starts_with(&prefix)) {
            continue;
        }
        agent_args.push(flag.to_string());
        agent_args.push(value.clone());
    }
    cfg.get("agent_fullscreen").map(|v| config_flag(v)).unwrap_or(false)
}

/// Size of the terminal latch-run itself is attached to (80x24 when detached).
fn outer_winsize() -> (u16, u16) {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let got = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0;
    if !got || ws.ws_row == 0 || ws.ws_col == 0 {
        return (24, 80);
    }
    (ws.ws_row, ws.ws_col)
}

/// Resize the agent's PTY; the kernel then sends the agent SIGWINCH.
fn set_pty_size(master_fd: RawFd, size: (u16, u16)) {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    ws.ws_row = size.0;
    ws.ws_col = size.1;
    unsafe {
        libc::ioctl(master_fd, libc::TIOCSWINSZ, &ws);
    }
}

/// True once the agent has put its terminal into raw mode, i.e. a TUI is
/// reading keys rather than lines. Always false for a pipe.
fn pty_is_raw(fd: RawFd) -> bool {
    let mut tio: libc::termios = unsafe { std::mem::zeroed() };
    unsafe { libc::tcgetattr(fd, &mut tio) == 0 && tio.c_lflag & libc::ICANON == 0 }
}

/// Send one line of input to the agent. A TUI needs Enter (CR) as its own
/// keypress to submit; a line-reading agent needs a newline.
fn send_line(w: &mut fs::File, text: &str) {
    if pty_is_raw(w.as_raw_fd()) {
        let _ = w.write_all(text.as_bytes());
        let _ = w.flush();
        thread::sleep(Duration::from_millis(150));
        let _ = w.write_all(b"\r");
    } else {
        let _ = writeln!(w, "{}", text);
    }
}

fn set_cloexec(fd: RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

/// Ask the agent (and its process group) to exit, escalating to SIGKILL if it
/// has not gone within two seconds.
fn stop_child(child: &mut Child) {
    let pid = child.id() as libc::pid_t;
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
        libc::kill(pid, libc::SIGTERM);
    }
    for _ in 0..20 {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Copy agent output to our stdout and watch it for needs-you markers.
fn spawn_reader(mut reader: fs::File, state: Arc<Mutex<RunnerState>>) {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    LAST_OUTPUT_MS.store(now_ms(), Ordering::Relaxed);
                    let mut out = std::io::stdout().lock();
                    let _ = out.write_all(&buf[..n]);
                    let _ = out.flush();
                    drop(out);
                    let text = String::from_utf8_lossy(&buf[..n]);
                    if text.contains("NEEDS_YOU")
                        || text.contains("[needs-you]")
                        || text.contains("waiting for user")
                    {
                        if let Ok(mut st) = state.lock() {
                            st.needs_you = true;
                            save_registry(&st);
                        }
                    }
                }
            }
        }
    });
}

/// When latch-run owns a real terminal (a tmux tab), switch it to raw mode and
/// pass keystrokes and terminal replies through to the agent, so its TUI is
/// usable. Returns the settings to restore on exit.
fn forward_terminal_input(writer: Arc<Mutex<fs::File>>) -> Option<libc::termios> {
    let fd = libc::STDIN_FILENO;
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::isatty(fd) } != 1 || unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
        return None;
    }
    let mut raw = saved;
    unsafe {
        libc::cfmakeraw(&mut raw);
        libc::tcsetattr(fd, libc::TCSANOW, &raw);
    }
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut stdin = std::io::stdin().lock();
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut w) = writer.lock() {
                        let _ = w.write_all(&buf[..n]);
                    }
                }
            }
        }
    });
    Some(saved)
}

fn common_md_path() -> PathBuf {
    let p1 = latchboard_config_dir().join("common.md");
    if p1.exists() {
        return p1;
    }
    home_dir().join(".config/latchd-up/common.md")
}

fn spawn_session_pty(spec: &TickSpec) -> Result<(Child, FilePair), String> {
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
        set_pty_size(master_fd, outer_winsize());
        set_cloexec(master_fd);
        set_cloexec(slave_fd);
        let slave_in = unsafe { Stdio::from_raw_fd( libc::dup(slave_fd) ) };
        let slave_out = unsafe { Stdio::from_raw_fd( libc::dup(slave_fd) ) };
        let slave_err = unsafe { Stdio::from_raw_fd(slave_fd) };

        let mut cmd = Command::new(&spec.runner_cmd);
        cmd.args(&spec.agent_args)
            .env("LATCH_SESSION", &spec.tick_id)
            .env("LATCH_TITLE", &spec.title)
            .env("LATCH_PRIO", prio)
            .stdin(slave_in)
            .stdout(slave_out)
            .stderr(slave_err);
        if spec.fullscreen {
            cmd.env("CLAUDE_CODE_NO_FLICKER", "1");
        }

        unsafe {
            cmd.pre_exec(|| {
                // Own session with the PTY as controlling terminal: the agent
                // then gets SIGWINCH on resize and SIGHUP if latch-run dies.
                libc::setsid();
                libc::ioctl(0, libc::TIOCSCTTY as _, 0);
                Ok(())
            });
        }

        let child = cmd.spawn().map_err(|e| e.to_string())?;
        let writer =
            unsafe { fs::File::from_raw_fd(libc::fcntl(master_fd, libc::F_DUPFD_CLOEXEC, 0)) };
        let reader = unsafe { fs::File::from_raw_fd(master_fd) };
        Ok((child, FilePair { reader, writer }))
    } else {
        let mut cmd = Command::new(&spec.runner_cmd);
        if spec.fullscreen {
            cmd.env("CLAUDE_CODE_NO_FLICKER", "1");
        }
        let mut child = cmd
            .args(&spec.agent_args)
            .env("LATCH_SESSION", &spec.tick_id)
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
    let reg_file = reg_dir.join(format!("{}.json", state.tick_id));
    if let Ok(txt) = serde_json::to_string_pretty(state) {
        let _ = fs::write(reg_file, txt);
    }
}

fn record_handover_note(tick_id: &str, reason: &str) {
    let inbox_dir = handover_state_dir().join("handover").join(tick_id);
    let _ = ensure_dir(&inbox_dir);
    let ts = now_epoch();
    let payload = json!({
        "ts": ts,
        "from": tick_id,
        "to": tick_id,
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
            "details": {"tick_id": tick_id, "reason": reason}
        }),
    );
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("Usage: latch-run <tick-id-or-path> [--once-init]");
        process::exit(2);
    }

    let tick_arg = &args[0];
    let Some(tick_path) = find_tick_file(tick_arg) else {
        eprintln!("tick file not found for: {}", tick_arg);
        process::exit(1);
    };

    let mut spec = match parse_tick_file(&tick_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to parse tick file: {}", e);
            process::exit(1);
        }
    };

    let state_dir = latchboard_state_dir();
    let sock_dir = state_dir.join("sockets");
    let _ = ensure_dir(&sock_dir);
    let sock_path = sock_dir.join(format!("{}.sock", spec.tick_id));
    let _ = fs::remove_file(&sock_path);

    let listener = match UnixListener::bind(&sock_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("failed to bind socket {:?}: {}", sock_path, e);
            process::exit(1);
        }
    };
    let _ = listener.set_nonblocking(true);

    unsafe {
        let handler = on_shutdown_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGHUP, handler);
    }

    let now = now_epoch();
    let initial_state = RunnerState {
        tick_id: spec.tick_id.clone(),
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
    println!("[latch-run] started tick '{}' ({}) | pid={} | every={:?}s | socket={}",
        initial_state.tick_id, initial_state.title, initial_state.pid,
        initial_state.every_secs, initial_state.socket_path);

    let shared_state = Arc::new(Mutex::new(initial_state));
    let pending_ticks = Arc::new(Mutex::new(0u32));
    let pending_restart: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let exe_path = env::current_exe().unwrap_or_else(|_| PathBuf::from(""));
    let mut tick_fp = file_fingerprint(&spec.path);
    let mut common_fp = file_fingerprint(&common_md_path());
    let mut config_fp = file_fingerprint(&config_file_path());
    let mut bin_fp = file_fingerprint(&exe_path);

    let (mut child, io_pair) = match spawn_session_pty(&spec) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("failed to spawn runner command '{}': {}", spec.runner_cmd, e);
            process::exit(1);
        }
    };

    let writer_arc = Arc::new(Mutex::new(io_pair.writer));
    spawn_reader(io_pair.reader, Arc::clone(&shared_state));
    let saved_termios = forward_terminal_input(Arc::clone(&writer_arc));

    // The prompt is held back until the agent is ready to take it.
    let mut prompt_due: Option<Instant> = Some(Instant::now());
    let mut pty_size = outer_winsize();
    let line_wait = line_prompt_wait();

    loop {
        if SHUTDOWN.load(Ordering::SeqCst) {
            break;
        }

        let size = outer_winsize();
        if size != pty_size {
            pty_size = size;
            if let Ok(w) = writer_arc.lock() {
                set_pty_size(w.as_raw_fd(), size);
            }
        }

        if let Some(since) = prompt_due {
            if let Ok(mut w) = writer_arc.lock() {
                let last = LAST_OUTPUT_MS.load(Ordering::Relaxed);
                let quiet_ms = (last > 0).then(|| now_ms() - last);
                if prompt_ready(pty_is_raw(w.as_raw_fd()), since.elapsed(), quiet_ms, line_wait) {
                    prompt_due = None;
                    if !spec.prompt.is_empty() {
                        send_line(&mut w, &spec.prompt);
                    }
                }
            }
        }

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
                    send_line(&mut w, &format!("TICK {}", now_ts));
                }
            }
            let mut st = shared_state.lock().unwrap();
            st.last_tick_ts = now_ts;
            save_registry(&st);
        }

        let new_tick_fp = file_fingerprint(&spec.path);
        let new_common_fp = file_fingerprint(&common_md_path());
        let new_config_fp = file_fingerprint(&config_file_path());
        let new_bin_fp = file_fingerprint(&exe_path);

        if (new_tick_fp != tick_fp || new_common_fp != common_fp || new_config_fp != config_fp)
            && spec.restart_on.iter().any(|r| r == "change")
        {
            tick_fp = new_tick_fp;
            common_fp = new_common_fp;
            config_fp = new_config_fp;
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
            record_handover_note(&spec.tick_id, &reason);
            stop_child(&mut child);
            if let Ok(updated) = parse_tick_file(&spec.path) {
                spec = updated;
            }
            if let Ok((new_child, new_io)) = spawn_session_pty(&spec) {
                child = new_child;
                if let Ok(mut w) = writer_arc.lock() {
                    *w = new_io.writer;
                }
                LAST_OUTPUT_MS.store(0, Ordering::Relaxed);
                spawn_reader(new_io.reader, Arc::clone(&shared_state));
                prompt_due = Some(Instant::now());
                let mut st = shared_state.lock().unwrap();
                st.title = spec.title.clone();
                st.every_secs = spec.every_secs;
                st.needs_you = false;
                save_registry(&st);
            }
        }

        thread::sleep(Duration::from_millis(100));
    }

    stop_child(&mut child);
    if let Some(saved) = saved_termios {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &saved);
        }
    }
    let _ = fs::remove_file(&sock_path);
    let _ = fs::remove_file(
        state_dir
            .join("runners")
            .join(format!("{}.json", spec.tick_id)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn config_adds_model_effort_and_fullscreen() {
        let mut args = Vec::new();
        let full = apply_agent_config(
            &cfg(&[("agent_model", "fable"), ("agent_effort", "low"), ("agent_fullscreen", "true")]),
            &mut args,
        );
        assert!(full);
        assert_eq!(args, ["--model", "fable", "--effort", "low"]);
    }

    #[test]
    fn tick_args_win_over_config() {
        let mut args = vec!["--effort=high".to_string()];
        let full = apply_agent_config(&cfg(&[("agent_model", "fable"), ("agent_effort", "low")]), &mut args);
        assert!(!full);
        assert_eq!(args, ["--effort=high", "--model", "fable"]);
    }

    #[test]
    fn prompt_waits_for_the_agent() {
        // line-based agent: fixed wait
        let w = PROMPT_WAIT;
        assert!(!prompt_ready(false, Duration::from_secs(1), Some(60_000), w));
        assert!(prompt_ready(false, PROMPT_WAIT, None, w));
        // TUI: once it has drawn and gone quiet, or eventually regardless
        assert!(!prompt_ready(true, Duration::from_secs(5), None, w));
        assert!(!prompt_ready(true, Duration::from_secs(5), Some(200), w));
        assert!(prompt_ready(true, Duration::from_secs(5), Some(TUI_QUIET_MS), w));
        assert!(prompt_ready(true, TUI_WAIT, None, w));
    }

    #[test]
    fn empty_config_changes_nothing() {
        let mut args = vec!["--verbose".to_string()];
        assert!(!apply_agent_config(&HashMap::new(), &mut args));
        assert_eq!(args, ["--verbose"]);
    }
}
