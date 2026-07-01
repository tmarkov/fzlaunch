use std::io;
use std::time::Duration;

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self as crossterm_terminal, Clear, ClearType};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};

use crate::app::Governor;
use crate::model::Value;
use crate::state::InputMode;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(30);
const RESULT_LIMIT: usize = 12;

pub async fn run(governor: &mut Governor) -> io::Result<Option<Value>> {
    let mut terminal = TerminalSession::enter()?;
    terminal.draw(governor)?;

    loop {
        let mut should_draw = governor.receive_pending_candidates() > 0;

        if event::poll(EVENT_POLL_INTERVAL)? {
            match event::read()? {
                Event::Key(key) => match handle_key(governor, key) {
                    KeyAction::Continue => should_draw = true,
                    KeyAction::Quit(command) => return Ok(command),
                },
                Event::Resize(_, _) => should_draw = true,
                _ => {}
            }
        }

        if should_draw {
            terminal.draw(governor)?;
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum KeyAction {
    Continue,
    Quit(Option<Value>),
}

fn handle_key(governor: &mut Governor, key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            KeyAction::Quit(None)
        }
        KeyCode::Esc => KeyAction::Quit(None),
        KeyCode::Enter => match governor.press_enter() {
            Some(command) => KeyAction::Quit(Some(command)),
            None => KeyAction::Continue,
        },
        KeyCode::Tab => {
            governor.press_tab();
            KeyAction::Continue
        }
        KeyCode::Up => {
            governor.select_previous();
            KeyAction::Continue
        }
        KeyCode::Down => {
            governor.select_next();
            KeyAction::Continue
        }
        KeyCode::Char('~') => {
            governor.press_tilde();
            KeyAction::Continue
        }
        KeyCode::Backspace => {
            update_text(governor, |text| {
                text.pop();
            });
            KeyAction::Continue
        }
        KeyCode::Char(ch) => {
            update_text(governor, |text| text.push(ch));
            KeyAction::Continue
        }
        _ => KeyAction::Continue,
    }
}

fn update_text(governor: &mut Governor, update: impl FnOnce(&mut String)) {
    let mut value = governor.value();
    update(&mut value.editable_text);
    governor.update_input(value);
}

fn render(frame: &mut Frame<'_>, governor: &Governor) {
    let input = governor.value();
    let mode = match governor.mode() {
        InputMode::Search => "search",
        InputMode::Edit => "edit",
    };
    let queue = governor.queue_status().unwrap_or_default();
    let current = governor.current().editable_text;
    let results = governor.results();
    let selected_index = governor.selected_index();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(frame.area());

    frame.render_widget(Paragraph::new(format!("fzlaunch [{mode}]")), chunks[0]);
    frame.render_widget(
        Paragraph::new(format!("> {}", input.editable_text)),
        chunks[1],
    );
    frame.render_widget(Paragraph::new(format!("queue: {queue}")), chunks[2]);
    frame.render_widget(Paragraph::new(format!("current: {current}")), chunks[3]);
    frame.render_widget(Paragraph::new("results"), chunks[4]);

    let items = results
        .into_iter()
        .take(RESULT_LIMIT)
        .map(|value| ListItem::new(value.editable_text))
        .collect::<Vec<_>>();
    let list = List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default();
    state.select(selected_index);
    frame.render_stateful_widget(list, chunks[5], &mut state);
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalSession {
    fn enter() -> io::Result<Self> {
        crossterm_terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            crossterm_terminal::EnterAlternateScreen,
            Clear(ClearType::All),
            cursor::MoveTo(0, 0),
            cursor::Hide
        )?;

        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        Ok(Self { terminal })
    }

    fn draw(&mut self, governor: &Governor) -> io::Result<()> {
        self.terminal.draw(|frame| render(frame, governor))?;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = execute!(
            self.terminal.backend_mut(),
            cursor::Show,
            crossterm_terminal::LeaveAlternateScreen
        );
        let _ = crossterm_terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn enter_with_incomplete_command_continues() {
        let mut governor = Governor::with_sources([]);

        governor.update_input(Value::raw("readlink -f {}"));

        assert_eq!(
            handle_key(&mut governor, key(KeyCode::Enter)),
            KeyAction::Continue
        );
        assert_eq!(governor.queue_status(), Some("readlink -f {}".into()));
    }

    #[test]
    fn enter_with_complete_command_quits_with_command() {
        let mut governor = Governor::with_sources([]);

        governor.update_input(Value::raw("nvim"));

        assert_eq!(
            handle_key(&mut governor, key(KeyCode::Enter)),
            KeyAction::Quit(Some(Value::raw("nvim")))
        );
    }
}
