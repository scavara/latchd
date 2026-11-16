//! Interaction logic for the dashboard: panel focus, the slash-command line
//! and confirmation. It decides what should happen; `main.rs` draws and runs
//! the resulting CLI call. The dashboard itself never writes runtime state.

use crossterm::event::KeyCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Overview,
    Rollouts,
    Runners,
    Notices,
    HostQueues,
    Directives,
    Maintenance,
    Activity,
}

impl Panel {
    /// The single key that focuses this panel.
    pub fn key(self) -> char {
        match self {
            Panel::Directives => 'd',
            Panel::Overview => 'o',
            Panel::Maintenance => 'm',
            Panel::HostQueues => 'h',
            Panel::Runners => 'w',
            Panel::Notices => 'n',
            Panel::Rollouts => 'r',
            Panel::Activity => 'a',
        }
    }

    pub fn for_key(c: char) -> Option<Panel> {
        [
            Panel::Directives,
            Panel::Overview,
            Panel::Maintenance,
            Panel::HostQueues,
            Panel::Runners,
            Panel::Notices,
            Panel::Rollouts,
            Panel::Activity,
        ]
        .into_iter()
        .find(|p| p.key() == c)
    }

    pub fn name(self) -> &'static str {
        match self {
            Panel::Overview => "Overview",
            Panel::Rollouts => "Rollouts",
            Panel::Runners => "Runners",
            Panel::Notices => "Notices",
            Panel::HostQueues => "Host queues",
            Panel::Directives => "Directives",
            Panel::Maintenance => "Maintenance",
            Panel::Activity => "Activity Feed",
        }
    }
}

/// A parsed slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Stop one tick (by tick id) or all of them.
    Shutdown(String),
    /// Mark a rollout (id or name) as done.
    Close(String),
    Help,
    Quit,
}

impl Command {
    /// The CLI invocation that carries the command out, as program name and
    /// arguments. Help and Quit are handled by the dashboard itself.
    pub fn cli(&self) -> Option<(&'static str, Vec<String>)> {
        match self {
            Command::Shutdown(target) => Some(("latch-up", vec!["stop".into(), target.clone()])),
            Command::Close(rollout) => Some((
                "latch-note",
                vec!["rollout".into(), "state".into(), rollout.clone(), "done".into()],
            )),
            Command::Help | Command::Quit => None,
        }
    }

    /// The question to confirm before running, for commands that change state.
    pub fn confirmation(&self) -> Option<String> {
        match self {
            Command::Shutdown(t) if t == "all" => Some("Stop ALL ticks? (y/n)".to_string()),
            Command::Shutdown(t) => Some(format!("Stop tick '{}'? (y/n)", t)),
            Command::Close(r) => Some(format!("Mark rollout '{}' as done? (y/n)", r)),
            Command::Help | Command::Quit => None,
        }
    }
}

pub const HELP: &str =
    "/shutdown N|<tick>|all  stop ticks   /close <rollout>  mark rollout done   /quit";

/// Parse the text typed after `/`. `runner_ids` are the tick ids in the order
/// the Runners panel numbers them, so `/shutdown 2` means the second one.
pub fn parse_command(input: &str, runner_ids: &[String]) -> Result<Command, String> {
    let mut words = input.trim().trim_start_matches('/').split_whitespace();
    let Some(name) = words.next() else {
        return Err("empty command (try /help)".to_string());
    };
    let arg = words.next();
    if words.next().is_some() {
        return Err(format!("/{}: too many arguments", name));
    }
    match (name, arg) {
        ("help" | "?", None) => Ok(Command::Help),
        ("quit" | "q", None) => Ok(Command::Quit),
        ("shutdown", Some("all")) => {
            if runner_ids.is_empty() {
                return Err("no ticks are running".to_string());
            }
            Ok(Command::Shutdown("all".to_string()))
        }
        ("shutdown", Some(target)) => {
            if let Ok(n) = target.parse::<usize>() {
                return match n.checked_sub(1).and_then(|i| runner_ids.get(i)) {
                    Some(id) => Ok(Command::Shutdown(id.clone())),
                    None => Err(format!("no runner #{} ({} running)", n, runner_ids.len())),
                };
            }
            if runner_ids.iter().any(|id| id == target) {
                Ok(Command::Shutdown(target.to_string()))
            } else {
                Err(format!("no running tick '{}'", target))
            }
        }
        ("shutdown", None) => Err("usage: /shutdown N|<tick>|all".to_string()),
        ("close", Some(rollout)) => Ok(Command::Close(rollout.to_string())),
        ("close", None) => Err("usage: /close <rollout id or name>".to_string()),
        (other, _) => Err(format!("unknown command /{} (try /help)", other)),
    }
}

/// What the dashboard should do after a key press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    /// Run this state-changing command through its CLI.
    Run(Command),
}

#[derive(Debug, Default)]
pub struct App {
    /// The panel shown full-size, if any.
    pub focus: Option<Panel>,
    /// Scroll offset inside the focused panel.
    pub scroll: u16,
    /// Text of the command line while one is being typed (without the `/`).
    pub input: Option<String>,
    /// A command waiting for y/n.
    pub confirm: Option<Command>,
    /// Result of the last command, or an error.
    pub message: Option<String>,
}

impl App {
    pub fn handle_key(&mut self, code: KeyCode, runner_ids: &[String]) -> Action {
        // 1. A pending confirmation takes the next key, whatever it is.
        if let Some(cmd) = self.confirm.take() {
            return match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => Action::Run(cmd),
                _ => {
                    self.message = Some("cancelled".to_string());
                    Action::None
                }
            };
        }

        // 2. Typing a command.
        if let Some(buf) = self.input.as_mut() {
            match code {
                KeyCode::Esc => self.input = None,
                KeyCode::Backspace => {
                    if buf.pop().is_none() {
                        self.input = None;
                    }
                }
                KeyCode::Char(c) => buf.push(c),
                KeyCode::Enter => {
                    let text = self.input.take().unwrap_or_default();
                    return self.submit(&text, runner_ids);
                }
                _ => {}
            }
            return Action::None;
        }

        // 3. Navigation.
        match code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('/') => {
                self.input = Some(String::new());
                self.message = None;
            }
            KeyCode::Esc => {
                self.focus = None;
                self.scroll = 0;
            }
            KeyCode::Char('j') | KeyCode::Down if self.focus.is_some() => {
                self.scroll = self.scroll.saturating_add(1);
            }
            KeyCode::Char('k') | KeyCode::Up if self.focus.is_some() => {
                self.scroll = self.scroll.saturating_sub(1);
            }
            KeyCode::PageDown if self.focus.is_some() => {
                self.scroll = self.scroll.saturating_add(10);
            }
            KeyCode::PageUp if self.focus.is_some() => {
                self.scroll = self.scroll.saturating_sub(10);
            }
            KeyCode::Char(c) => {
                if let Some(panel) = Panel::for_key(c) {
                    // The same key again returns to the grid.
                    self.focus = if self.focus == Some(panel) { None } else { Some(panel) };
                    self.scroll = 0;
                }
            }
            _ => {}
        }
        Action::None
    }

    fn submit(&mut self, text: &str, runner_ids: &[String]) -> Action {
        match parse_command(text, runner_ids) {
            Err(e) => {
                self.message = Some(e);
                Action::None
            }
            Ok(Command::Quit) => Action::Quit,
            Ok(Command::Help) => {
                self.message = Some(HELP.to_string());
                Action::None
            }
            Ok(cmd) => {
                self.confirm = Some(cmd);
                Action::None
            }
        }
    }

    /// Keep the scroll offset inside a panel of `lines` lines shown in a
    /// window `height` lines tall.
    pub fn clamp_scroll(&mut self, lines: usize, height: usize) {
        let max = lines.saturating_sub(height).min(u16::MAX as usize) as u16;
        if self.scroll > max {
            self.scroll = max;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Vec<String> {
        vec!["cleanup".to_string(), "incident".to_string(), "telemetry".to_string()]
    }

    fn type_line(app: &mut App, text: &str) -> Action {
        assert_eq!(app.handle_key(KeyCode::Char('/'), &ids()), Action::None);
        for c in text.chars() {
            app.handle_key(KeyCode::Char(c), &ids());
        }
        app.handle_key(KeyCode::Enter, &ids())
    }

    #[test]
    fn roadmap_keys_focus_their_panels() {
        for (key, panel) in [
            ('d', Panel::Directives),
            ('o', Panel::Overview),
            ('m', Panel::Maintenance),
            ('h', Panel::HostQueues),
            ('w', Panel::Runners),
            ('n', Panel::Notices),
        ] {
            assert_eq!(Panel::for_key(key), Some(panel));
        }
        assert_eq!(Panel::for_key('x'), None);
    }

    #[test]
    fn parses_shutdown() {
        let ids = ids();
        assert_eq!(parse_command("shutdown 2", &ids), Ok(Command::Shutdown("incident".into())));
        assert_eq!(parse_command("/shutdown telemetry", &ids), Ok(Command::Shutdown("telemetry".into())));
        assert_eq!(parse_command(" shutdown  all ", &ids), Ok(Command::Shutdown("all".into())));
        assert!(parse_command("shutdown 0", &ids).is_err());
        assert!(parse_command("shutdown 4", &ids).is_err());
        assert!(parse_command("shutdown nope", &ids).is_err());
        assert!(parse_command("shutdown", &ids).is_err());
        assert!(parse_command("shutdown 1 2", &ids).is_err());
        assert!(parse_command("shutdown all", &[]).is_err());
    }

    #[test]
    fn parses_close_and_others() {
        assert_eq!(parse_command("close 3", &[]), Ok(Command::Close("3".into())));
        assert_eq!(parse_command("close bios-update", &[]), Ok(Command::Close("bios-update".into())));
        assert!(parse_command("close", &[]).is_err());
        assert_eq!(parse_command("help", &[]), Ok(Command::Help));
        assert_eq!(parse_command("q", &[]), Ok(Command::Quit));
        assert!(parse_command("", &[]).is_err());
        assert!(parse_command("reboot now", &[]).is_err());
    }

    #[test]
    fn commands_go_through_the_clis() {
        assert_eq!(
            Command::Shutdown("all".into()).cli(),
            Some(("latch-up", vec!["stop".to_string(), "all".to_string()]))
        );
        assert_eq!(
            Command::Close("7".into()).cli(),
            Some((
                "latch-note",
                vec!["rollout".to_string(), "state".to_string(), "7".to_string(), "done".to_string()]
            ))
        );
        assert_eq!(Command::Quit.cli(), None);
    }

    #[test]
    fn focus_toggle_scroll_and_escape() {
        let mut app = App::default();
        // scrolling does nothing in the grid
        app.handle_key(KeyCode::Char('j'), &[]);
        assert_eq!((app.focus, app.scroll), (None, 0));

        app.handle_key(KeyCode::Char('w'), &[]);
        assert_eq!(app.focus, Some(Panel::Runners));
        app.handle_key(KeyCode::Char('j'), &[]);
        app.handle_key(KeyCode::Down, &[]);
        app.handle_key(KeyCode::Char('k'), &[]);
        assert_eq!(app.scroll, 1);
        app.clamp_scroll(5, 10);
        assert_eq!(app.scroll, 0);

        // another panel key switches, the same key again closes
        app.handle_key(KeyCode::Char('n'), &[]);
        assert_eq!(app.focus, Some(Panel::Notices));
        app.handle_key(KeyCode::Char('n'), &[]);
        assert_eq!(app.focus, None);

        app.handle_key(KeyCode::Char('d'), &[]);
        app.handle_key(KeyCode::Esc, &[]);
        assert_eq!(app.focus, None);
        assert_eq!(app.handle_key(KeyCode::Char('q'), &[]), Action::Quit);
    }

    #[test]
    fn shutdown_needs_confirmation() {
        let mut app = App::default();
        assert_eq!(type_line(&mut app, "shutdown 1"), Action::None);
        assert_eq!(app.confirm, Some(Command::Shutdown("cleanup".into())));
        assert_eq!(
            app.handle_key(KeyCode::Char('y'), &ids()),
            Action::Run(Command::Shutdown("cleanup".into()))
        );
        assert_eq!(app.confirm, None);

        // anything but y cancels, and the key is not treated as navigation
        assert_eq!(type_line(&mut app, "shutdown all"), Action::None);
        assert_eq!(app.handle_key(KeyCode::Char('q'), &ids()), Action::None);
        assert_eq!(app.confirm, None);
        assert_eq!(app.message.as_deref(), Some("cancelled"));
    }

    #[test]
    fn typing_does_not_trigger_shortcuts() {
        let mut app = App::default();
        // "q" and panel letters are plain text while typing a command
        assert_eq!(type_line(&mut app, "quux d"), Action::None);
        assert_eq!(app.focus, None);
        assert!(app.message.as_deref().unwrap().contains("unknown command /quux"));

        // escape and backspace-on-empty both leave the command line
        app.handle_key(KeyCode::Char('/'), &[]);
        app.handle_key(KeyCode::Char('x'), &[]);
        app.handle_key(KeyCode::Esc, &[]);
        assert_eq!(app.input, None);
        app.handle_key(KeyCode::Char('/'), &[]);
        app.handle_key(KeyCode::Backspace, &[]);
        assert_eq!(app.input, None);

        assert_eq!(type_line(&mut app, "quit"), Action::Quit);
        assert_eq!(type_line(&mut app, "help"), Action::None);
        assert_eq!(app.message.as_deref(), Some(HELP));
    }
}
