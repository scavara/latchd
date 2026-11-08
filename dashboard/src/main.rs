mod state;

use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
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
            println!("  {} [{}]", rn["loop_id"], rn["title"]);
        }
        return Ok(arguments_exit());
    }

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(5)])
                .split(f.size());

            let title_p = Paragraph::new(" latchboard dashboard (press q to quit)")
                .block(Block::default().borders(Borders::ALL).title("Status"));
            f.render_widget(title_p, chunks[0]);

            let snap_data = state::load_snapshot();
            let mut content = format!(
                "Claims: {}\nRollouts: {}\nRunners: {}\nNotices: {}\n\n",
                snap_data.claims.len(),
                snap_data.rollouts.len(),
                snap_data.runners.len(),
                snap_data.notices.len()
            );
            for c in &snap_data.claims {
                content.push_str(&format!("  [Claim] {} held by {} (p{})\n", c.node, c.holder, c.prio));
            }
            for r in &snap_data.rollouts {
                content.push_str(&format!("  [Rollout #{}] {} ({}/{})\n", r.id, r.name, r.state, r.phase));
            }

            let body_p = Paragraph::new(content)
                .block(Block::default().borders(Borders::ALL).title("Overview"));
            f.render_widget(body_p, chunks[1]);
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
