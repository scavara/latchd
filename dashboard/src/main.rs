mod state;

use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use std::env;
use std::io::{self, stdout};

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let snap = state::load_snapshot();

    if args.iter().any(|a| a == "--once") {
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
        return Ok(arguments_exit());
    }

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let border_style = Style::default().fg(Color::Cyan);
    let title_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    loop {
        terminal.draw(|f| {
            let snap_data = state::load_snapshot();

            // Main vertical layout: Header, Top Overview, Middle 3-col, Bottom 3-col
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Percentage(28),
                    Constraint::Percentage(36),
                    Constraint::Percentage(36),
                ])
                .split(f.size());

            // 1. Status Header
            let status_text = format!(
                " LATCHBOARD DASHBOARD | Claims: {} | Rollouts: {} | Runners: {} | Notices: {} (press q to quit)",
                snap_data.claims.len(),
                snap_data.rollouts.len(),
                snap_data.runners.len(),
                snap_data.notices.len()
            );
            let title_p = Paragraph::new(status_text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(ratatui::text::Span::styled(" Status ", title_style)),
            );
            f.render_widget(title_p, rows[0]);

            // 2. Overview Row
            let mut overview_text = format!("Claims ({}):\n", snap_data.claims.len());
            if snap_data.claims.is_empty() {
                overview_text.push_str("  (no active host claims)\n");
            } else {
                for c in &snap_data.claims {
                    overview_text.push_str(&format!("  {} -> {} (p{})\n", c.node, c.holder, c.prio));
                }
            }
            let overview_p = Paragraph::new(overview_text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(ratatui::text::Span::styled(" Overview ", title_style)),
            );
            f.render_widget(overview_p, rows[1]);

            // 3. Middle Row: Rollouts | Runners | Notices
            let mid_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(30),
                    Constraint::Percentage(40),
                    Constraint::Percentage(30),
                ])
                .split(rows[2]);

            // 3a. Rollouts
            let mut rollouts_text = String::new();
            if snap_data.rollouts.is_empty() {
                rollouts_text.push_str("  No active rollouts\n");
            } else {
                for r in &snap_data.rollouts {
                    rollouts_text.push_str(&format!("  #{} {} [{}/{}]\n", r.id, r.name, r.state, r.phase));
                }
            }
            let rollouts_p = Paragraph::new(rollouts_text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(ratatui::text::Span::styled(
                        format!(" Rollouts ({}) ", snap_data.rollouts.len()),
                        title_style,
                    )),
            );
            f.render_widget(rollouts_p, mid_cols[0]);

            // 3b. Runners
            let mut runners_text = String::from("  ID                 STATE      TITLE\n");
            if snap_data.runners.is_empty() {
                runners_text.push_str("  (no active runners)\n");
            } else {
                for rn in &snap_data.runners {
                    let id = rn["tick_id"].as_str().unwrap_or("-");
                    let st = rn["state"].as_str().unwrap_or("active");
                    let title = rn["title"].as_str().unwrap_or("-");
                    runners_text.push_str(&format!("  {:<18} {:<10} {}\n", id, st, title));
                }
            }
            let runners_p = Paragraph::new(runners_text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(ratatui::text::Span::styled(
                        format!(" Runners ({}) ", snap_data.runners.len()),
                        title_style,
                    )),
            );
            f.render_widget(runners_p, mid_cols[1]);

            // 3c. Notices
            let mut notices_text = String::new();
            if snap_data.notices.is_empty() {
                notices_text.push_str("  No active notices\n");
            } else {
                for n in &snap_data.notices {
                    notices_text.push_str(&format!("  {}\n", n));
                }
            }
            let notices_p = Paragraph::new(notices_text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(ratatui::text::Span::styled(
                        format!(" Notices ({}) ", snap_data.notices.len()),
                        title_style,
                    )),
            );
            f.render_widget(notices_p, mid_cols[2]);

            // 4. Bottom Row: Host queues | Directives | Active Details
            let bot_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(35),
                    Constraint::Percentage(25),
                    Constraint::Percentage(40),
                ])
                .split(rows[3]);

            let mut hq_text = String::new();
            if snap_data.claims.is_empty() {
                hq_text.push_str("  All host queues idle\n");
            } else {
                for c in &snap_data.claims {
                    hq_text.push_str(&format!("  {:<20} locked by {} (p{})\n", c.node, c.holder, c.prio));
                }
            }
            let hq_p = Paragraph::new(hq_text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(ratatui::text::Span::styled(" Host queues ", title_style)),
            );
            f.render_widget(hq_p, bot_cols[0]);

            let directives_p = Paragraph::new("  Directives active: 0\n  prio_floor: none\n").block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(ratatui::text::Span::styled(" Directives ", title_style)),
            );
            f.render_widget(directives_p, bot_cols[1]);

            let mut feed_text = String::new();
            for c in &snap_data.claims {
                feed_text.push_str(&format!("  [Claim] {} -> {} (prio {})\n", c.node, c.holder, c.prio));
            }
            for n in &snap_data.notices {
                feed_text.push_str(&format!("  [Notice] {}\n", n));
            }
            if feed_text.is_empty() {
                feed_text.push_str("  Waiting for tick activity...\n");
            }
            let feed_p = Paragraph::new(feed_text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(ratatui::text::Span::styled(" Activity Feed ", title_style)),
            );
            f.render_widget(feed_p, bot_cols[2]);
        })?;

        if event::poll(std::time::Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if let KeyCode::Char('q') = key.code {
                    break;
                }
            }
        }
    }

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

fn arguments_exit() {}
