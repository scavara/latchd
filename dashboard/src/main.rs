mod app;
mod state;

use app::{Action, App, Command, Panel};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use latch_core::now_epoch;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use state::{DirectiveEntry, Snapshot};
use std::env;
use std::io::{self, stdout};
use std::path::PathBuf;
use std::process::{self, Stdio};

/// Colour band for a claim priority: incident-level red, elevated yellow,
/// normal green, low blue.
fn prio_color(prio: u8) -> Color {
    match prio {
        8..=u8::MAX => Color::Red,
        6..=7 => Color::Yellow,
        4..=5 => Color::Green,
        _ => Color::Blue,
    }
}

fn prio_style(prio: u8) -> Style {
    let style = Style::default().fg(prio_color(prio));
    if prio >= 8 {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

/// One claim as a line whose priority tag (and node name) carry the colour.
fn claim_line(c: &state::ClaimEntry, joiner: &str, node_width: usize) -> Line<'static> {
    let style = prio_style(c.prio);
    Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{:<width$}", c.node, width = node_width), style),
        Span::raw(format!(" {} {} ", joiner, c.holder)),
        Span::styled(format!("(p{})", c.prio), style),
    ])
}

/// A span of seconds as `45s`, `12m`, `3h05m` or `2d04h`.
fn fmt_span(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        format!("{}s", s)
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86400 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}d{:02}h", s / 86400, (s % 86400) / 3600)
    }
}

fn directive_line(d: &DirectiveEntry, now: i64) -> String {
    let when = if d.is_upcoming(now) {
        format!("starts in {}", fmt_span(d.from_ts.unwrap_or(now) - now))
    } else {
        format!("ends in {}", fmt_span(d.until_ts - now))
    };
    let text = d.text.strip_prefix("maintenance:").unwrap_or(&d.text).trim();
    let sync = if d.synced { " [synced]" } else { "" };
    format!("  #{} {} on {} ({}){}: {}", d.id, d.mode, d.nodes.join(","), when, sync, text)
}

/// How a runner registry entry reads on screen.
fn runner_state(rn: &serde_json::Value) -> &'static str {
    if rn["paused"].as_bool() == Some(true) {
        "paused"
    } else if rn["needs_you"].as_bool() == Some(true) {
        "needs-you"
    } else {
        "running"
    }
}

fn notice_line(n: &serde_json::Value) -> String {
    match n["title"].as_str() {
        Some(title) => format!("  [{}] {}", n["provider"].as_str().unwrap_or("?"), title),
        None => format!("  {}", n),
    }
}

/// The full content of a panel. The grid shows the top of it; a focused panel
/// shows all of it with scrolling.
fn panel_lines(panel: Panel, snap: &Snapshot, now: i64) -> Vec<Line<'static>> {
    let plain = |lines: Vec<String>, empty: &str| -> Vec<Line<'static>> {
        if lines.is_empty() {
            vec![Line::from(format!("  {}", empty))]
        } else {
            lines.into_iter().map(Line::from).collect()
        }
    };
    match panel {
        Panel::Overview => {
            let mut lines = vec![Line::from(format!("Claims ({}):", snap.claims.len()))];
            if snap.claims.is_empty() {
                lines.push(Line::from("  (no active host claims)"));
            }
            lines.extend(snap.claims.iter().map(|c| claim_line(c, "->", 0)));
            lines
        }
        Panel::HostQueues => {
            if snap.claims.is_empty() {
                return vec![Line::from("  All host queues idle")];
            }
            snap.claims.iter().map(|c| claim_line(c, "locked by", 20)).collect()
        }
        Panel::Rollouts => plain(
            snap.rollouts
                .iter()
                .map(|r| format!("  #{} {} [{}/{}]", r.id, r.name, r.state, r.phase))
                .collect(),
            "No rollouts",
        ),
        Panel::Runners => {
            let mut lines = vec![Line::from("   #  ID                 STATE      TITLE")];
            if snap.runners.is_empty() {
                lines.push(Line::from("  (no active runners)"));
            }
            for (i, rn) in snap.runners.iter().enumerate() {
                lines.push(Line::from(format!(
                    "  {:>2}  {:<18} {:<10} {}",
                    i + 1,
                    rn["tick_id"].as_str().unwrap_or("-"),
                    runner_state(rn),
                    rn["title"].as_str().unwrap_or("-")
                )));
            }
            lines
        }
        Panel::Notices => plain(snap.notices.iter().map(notice_line).collect(), "No active notices"),
        Panel::Directives => plain(
            snap.plain_directives().iter().map(|d| directive_line(d, now)).collect(),
            "No active directives",
        ),
        Panel::Maintenance => plain(
            snap.maintenance().iter().map(|d| directive_line(d, now)).collect(),
            "No maintenance windows",
        ),
        Panel::Activity => {
            let mut lines: Vec<String> = snap
                .claims
                .iter()
                .map(|c| format!("  [Claim] {} -> {} (prio {})", c.node, c.holder, c.prio))
                .collect();
            lines.extend(snap.notices.iter().map(|n| format!("  [Notice]{}", &notice_line(n)[1..])));
            plain(lines, "Waiting for tick activity...")
        }
    }
}

fn panel_count(panel: Panel, snap: &Snapshot) -> Option<usize> {
    match panel {
        Panel::Rollouts => Some(snap.rollouts.len()),
        Panel::Runners => Some(snap.runners.len()),
        Panel::Notices => Some(snap.notices.len()),
        Panel::Directives => Some(snap.plain_directives().len()),
        Panel::Maintenance => Some(snap.maintenance().len()),
        Panel::Overview | Panel::HostQueues | Panel::Activity => None,
    }
}

fn draw_panel(f: &mut Frame, area: Rect, panel: Panel, snap: &Snapshot, now: i64, app: &mut App) {
    let focused = app.focus == Some(panel);
    let lines = panel_lines(panel, snap, now);
    let color = if focused { Color::Yellow } else { Color::Cyan };
    let count = panel_count(panel, snap).map(|n| format!(" ({})", n)).unwrap_or_default();
    let title = format!(" {}{} [{}] ", panel.name(), count, panel.key());
    let mut paragraph = Paragraph::new(lines.clone()).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(color))
            .title(Span::styled(title, Style::default().fg(color).add_modifier(Modifier::BOLD))),
    );
    if focused {
        app.clamp_scroll(lines.len(), area.height.saturating_sub(2) as usize);
        paragraph = paragraph.scroll((app.scroll, 0));
    }
    f.render_widget(paragraph, area);
}

/// The bottom line: the command being typed, a confirmation, the last result,
/// or the key hints.
fn footer_line(app: &App) -> Line<'static> {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    if let Some(cmd) = &app.confirm {
        return Line::from(Span::styled(
            format!(" {}", cmd.confirmation().unwrap_or_default()),
            bold.fg(Color::Yellow),
        ));
    }
    if let Some(input) = &app.input {
        return Line::from(vec![
            Span::styled(format!(" /{}", input), bold),
            Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
            Span::styled("   Enter run · Esc cancel", Style::default().fg(Color::DarkGray)),
        ]);
    }
    if let Some(msg) = &app.message {
        return Line::from(format!(" {}", msg));
    }
    let hint = if app.focus.is_some() {
        " j/k scroll · Esc back · / command · q quit"
    } else {
        " d o m h w n r a focus a panel · / command (/help) · q quit"
    };
    Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray)))
}

fn draw(f: &mut Frame, snap: &Snapshot, now: i64, app: &mut App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3), Constraint::Length(1)])
        .split(f.size());

    let status_text = format!(
        " LATCHBOARD DASHBOARD | Claims: {} | Rollouts: {} | Runners: {} | Notices: {} | Directives: {} | Maintenance: {}",
        snap.claims.len(),
        snap.rollouts.len(),
        snap.runners.len(),
        snap.notices.len(),
        snap.plain_directives().len(),
        snap.maintenance().len()
    );
    let cyan = Style::default().fg(Color::Cyan);
    f.render_widget(
        Paragraph::new(status_text).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(cyan)
                .title(Span::styled(" Status ", cyan.add_modifier(Modifier::BOLD))),
        ),
        rows[0],
    );
    f.render_widget(Paragraph::new(footer_line(app)), rows[2]);

    // A focused panel takes the whole body.
    if let Some(panel) = app.focus {
        draw_panel(f, rows[1], panel, snap, now, app);
        return;
    }

    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(28),
            Constraint::Percentage(36),
            Constraint::Percentage(36),
        ])
        .split(rows[1]);
    draw_panel(f, body[0], Panel::Overview, snap, now, app);

    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(40),
            Constraint::Percentage(30),
        ])
        .split(body[1]);
    draw_panel(f, mid[0], Panel::Rollouts, snap, now, app);
    draw_panel(f, mid[1], Panel::Runners, snap, now, app);
    draw_panel(f, mid[2], Panel::Notices, snap, now, app);

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(27),
            Constraint::Percentage(24),
            Constraint::Percentage(24),
            Constraint::Percentage(25),
        ])
        .split(body[2]);
    draw_panel(f, bottom[0], Panel::HostQueues, snap, now, app);
    draw_panel(f, bottom[1], Panel::Directives, snap, now, app);
    draw_panel(f, bottom[2], Panel::Maintenance, snap, now, app);
    draw_panel(f, bottom[3], Panel::Activity, snap, now, app);
}

/// Where a companion CLI lives: an explicit override, next to this binary,
/// or on PATH.
fn find_cli(name: &str) -> PathBuf {
    let var = format!("{}_BIN", name.to_uppercase().replace('-', "_"));
    if let Ok(path) = env::var(var) {
        return PathBuf::from(path);
    }
    if let Some(sibling) = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|p| p.exists())
    {
        return sibling;
    }
    PathBuf::from(name)
}

/// Carry out a slash command through its CLI and report the outcome in one
/// line. The dashboard never writes runtime state itself.
fn run_command(cmd: &Command) -> String {
    let Some((program, args)) = cmd.cli() else {
        return String::new();
    };
    let output = process::Command::new(find_cli(program))
        .args(&args)
        .stdin(Stdio::null())
        .output();
    match output {
        Err(e) => format!("could not run {}: {}", program, e),
        Ok(out) => {
            let text = if out.status.success() { &out.stdout } else { &out.stderr };
            let text = String::from_utf8_lossy(text);
            let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
            if !out.status.success() {
                // The last line of an error is the one that says what went wrong.
                return match lines.last() {
                    Some(last) => format!("error: {}", last),
                    None => format!("error: {} failed ({})", program, out.status),
                };
            }
            match lines.as_slice() {
                [] => "nothing to do".to_string(),
                [one] => one.to_string(),
                [first, rest @ ..] => format!("{} (+{} more)", first, rest.len()),
            }
        }
    }
}

fn print_once(snap: &Snapshot, now: i64) {
    println!("=== LATCHBOARD SNAPSHOT ===");
    println!("Claims ({}):", snap.claims.len());
    for c in &snap.claims {
        println!("  {} -> {} (p{})", c.node, c.holder, c.prio);
    }
    println!("Rollouts ({}):", snap.rollouts.len());
    for r in &snap.rollouts {
        println!("  #{} {} [{}/{}]", r.id, r.name, r.state, r.phase);
    }
    println!("Runners ({}):", snap.runners.len());
    for rn in &snap.runners {
        println!("  {} [{}]", rn["tick_id"], rn["title"]);
    }
    println!("Directives ({}):", snap.plain_directives().len());
    for d in snap.plain_directives() {
        println!("{}", directive_line(d, now));
    }
    println!("Maintenance ({}):", snap.maintenance().len());
    for d in snap.maintenance() {
        println!("{}", directive_line(d, now));
    }
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.iter().any(|a| a == "--once") {
        print_once(&state::load_snapshot(), now_epoch());
        return Ok(());
    }

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::default();

    loop {
        let snap = state::load_snapshot();
        let now = now_epoch();
        terminal.draw(|f| draw(f, &snap, now, &mut app))?;

        if !event::poll(std::time::Duration::from_millis(200))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.handle_key(key.code, &snap.runner_ids()) {
            Action::None => {}
            Action::Quit => break,
            Action::Run(cmd) => app.message = Some(run_command(&cmd)),
        }
    }

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_bands() {
        assert_eq!(prio_color(9), Color::Red);
        assert_eq!(prio_color(8), Color::Red);
        assert_eq!(prio_color(7), Color::Yellow);
        assert_eq!(prio_color(6), Color::Yellow);
        assert_eq!(prio_color(5), Color::Green);
        assert_eq!(prio_color(4), Color::Green);
        assert_eq!(prio_color(3), Color::Blue);
        assert_eq!(prio_color(1), Color::Blue);
        assert!(prio_style(9).add_modifier.contains(Modifier::BOLD));
        assert!(!prio_style(7).add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn claim_line_text_and_colour() {
        let c = state::ClaimEntry { node: "ab1001".into(), holder: "tick-a".into(), prio: 7 };
        let line = claim_line(&c, "->", 0);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "  ab1001 -> tick-a (p7)");
        assert_eq!(line.spans[3].style.fg, Some(Color::Yellow));
    }

    #[test]
    fn spans_and_directive_lines() {
        assert_eq!(fmt_span(45), "45s");
        assert_eq!(fmt_span(12 * 60 + 5), "12m");
        assert_eq!(fmt_span(3 * 3600 + 5 * 60), "3h05m");
        assert_eq!(fmt_span(2 * 86400 + 4 * 3600), "2d04h");
        assert_eq!(fmt_span(-5), "0s");

        let mut d = DirectiveEntry {
            id: 4,
            text: "maintenance: PSU swap".into(),
            nodes: vec!["ab1001".into(), "ab1002".into()],
            mode: "block".into(),
            from_ts: Some(1600),
            until_ts: 5000,
            synced: false,
        };
        assert_eq!(directive_line(&d, 1000), "  #4 block on ab1001,ab1002 (starts in 10m): PSU swap");
        d.synced = true;
        assert_eq!(
            directive_line(&d, 2000),
            "  #4 block on ab1001,ab1002 (ends in 50m) [synced]: PSU swap"
        );
    }

    #[test]
    fn runner_states_and_numbering() {
        let snap = Snapshot {
            runners: vec![
                serde_json::json!({"tick_id": "cleanup", "title": "Cleanup", "paused": true}),
                serde_json::json!({"tick_id": "incident", "title": "Incident", "needs_you": true}),
                serde_json::json!({"tick_id": "telemetry", "title": "Telemetry"}),
            ],
            ..Snapshot::default()
        };
        let text: Vec<String> = panel_lines(Panel::Runners, &snap, 0)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(text[1].starts_with("   1  cleanup            paused"));
        assert!(text[2].starts_with("   2  incident           needs-you"));
        assert!(text[3].starts_with("   3  telemetry          running"));
        assert_eq!(snap.runner_ids(), ["cleanup", "incident", "telemetry"]);
    }
}
