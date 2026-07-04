use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self as crossterm_terminal, Clear, ClearType};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::app::App;
use crate::model::Value;
use crate::preview::Preview;
use crate::state::{InputMode, ResultRow};
use tokio::sync::mpsc;

const EVENT_CHANNEL_CAPACITY: usize = 16;
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(16);
const BLOCKING_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);
const RESULT_HIGHLIGHT_SYMBOL: &str = "> ";

pub async fn run(app: &mut App) -> io::Result<Option<Value>> {
    let mut terminal = TerminalSession::enter()?;
    let mut events = TerminalEvents::start();
    app.refresh_preview();
    terminal.draw(app)?;

    loop {
        let mut should_draw = app.receive_pending_candidates() > 0;
        should_draw |= app.receive_pending_preview();

        tokio::select! {
            event = events.recv() => {
                match event? {
                    Some(Event::Key(key)) => match handle_key(app, key) {
                        KeyAction::Continue => should_draw = true,
                        KeyAction::Quit(command) => return Ok(command),
                    },
                    Some(Event::Resize(_, _)) => should_draw = true,
                    Some(_) => {}
                    None => return Ok(None),
                }
            }
            _ = tokio::time::sleep(EVENT_POLL_INTERVAL) => {}
        }

        if should_draw {
            app.refresh_preview();
            terminal.draw(app)?;
        }
    }
}

struct TerminalEvents {
    receiver: mpsc::Receiver<io::Result<Event>>,
    stop: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl TerminalEvents {
    fn start() -> Self {
        let (sender, receiver) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let task_stop = Arc::clone(&stop);
        let task = tokio::task::spawn_blocking(move || loop {
            if task_stop.load(Ordering::Relaxed) {
                break;
            }

            match event::poll(BLOCKING_EVENT_POLL_INTERVAL) {
                Ok(true) => {
                    let event = event::read();
                    let should_stop = event.is_err();
                    if sender.blocking_send(event).is_err() || should_stop {
                        break;
                    }
                }
                Ok(false) => {}
                Err(error) => {
                    let _ = sender.blocking_send(Err(error));
                    break;
                }
            }
        });

        Self {
            receiver,
            stop,
            task,
        }
    }

    async fn recv(&mut self) -> io::Result<Option<Event>> {
        self.receiver.recv().await.transpose()
    }
}

impl Drop for TerminalEvents {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.task.abort();
    }
}

#[derive(Debug, PartialEq, Eq)]
enum KeyAction {
    Continue,
    Quit(Option<Value>),
}

fn handle_key(app: &mut App, key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            KeyAction::Quit(None)
        }
        KeyCode::Esc => KeyAction::Quit(None),
        KeyCode::Enter => match app.press_enter() {
            Some(command) => KeyAction::Quit(Some(command)),
            None => KeyAction::Continue,
        },
        KeyCode::Tab => {
            app.press_tab();
            KeyAction::Continue
        }
        KeyCode::Up => {
            app.select_previous();
            KeyAction::Continue
        }
        KeyCode::Down => {
            app.select_next();
            KeyAction::Continue
        }
        KeyCode::Char('~') => {
            app.press_tilde();
            KeyAction::Continue
        }
        KeyCode::Backspace => {
            update_text(app, |text| {
                text.pop();
            });
            KeyAction::Continue
        }
        KeyCode::Char(ch) => {
            update_text(app, |text| text.push(ch));
            KeyAction::Continue
        }
        _ => KeyAction::Continue,
    }
}

fn update_text(app: &mut App, update: impl FnOnce(&mut String)) {
    let mut value = app.state().value();
    value.edit_text(update);
    app.update_input(value);
}

fn render(frame: &mut Frame<'_>, app: &App) {
    let state = app.state();
    let input = state.value();
    let mode = match state.mode() {
        InputMode::Search => "search",
        InputMode::Edit => "edit",
    };
    let queue = state.queue_status().unwrap_or_default();
    let results = state.results();
    let selected_index = state.selected_index();

    let area = frame.area();
    let shell = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::DarkGray))
        .title(Line::from(vec![
            Span::styled(" fzlaunch ", Style::new().add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {mode} "), mode_style(state.mode())),
        ]));
    let shell_area = shell.inner(area);
    frame.render_widget(shell, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Min(0),
        ])
        .split(shell_area);

    render_input(
        frame,
        chunks[0],
        input.editable_text().to_string(),
        state.mode(),
    );
    render_queue(frame, chunks[1], queue);
    render_result_area(frame, chunks[2], results, selected_index, app.preview());
}

fn render_input(frame: &mut Frame<'_>, area: Rect, input: String, mode: InputMode) {
    let accent = mode_style(mode);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(accent)
        .title(Span::styled(" input ", accent));
    let line = Line::from(vec![
        Span::styled("> ", accent.add_modifier(Modifier::BOLD)),
        Span::raw(input),
    ]);

    frame.render_widget(Paragraph::new(line).block(block), area);
}

fn render_queue(frame: &mut Frame<'_>, area: Rect, text: String) {
    let empty = text.is_empty();
    let text = if empty { "empty".to_string() } else { text };
    let style = if empty {
        Style::new().fg(Color::DarkGray)
    } else {
        Style::new()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::DarkGray))
        .title(Span::styled(" queue ", Style::new().fg(Color::Gray)));

    frame.render_widget(
        Paragraph::new(text)
            .style(style)
            .block(block)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_result_area(
    frame: &mut Frame<'_>,
    area: Rect,
    results: Vec<ResultRow>,
    selected_index: Option<usize>,
    preview: &Preview,
) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(2, 3), Constraint::Ratio(1, 3)])
        .split(area);

    render_results(frame, chunks[0], results, selected_index);
    render_preview(frame, chunks[1], preview);
}

fn render_results(
    frame: &mut Frame<'_>,
    area: Rect,
    results: Vec<ResultRow>,
    selected_index: Option<usize>,
) {
    let total = results.len();
    let visible_count = area.height.saturating_sub(2).max(1) as usize;
    let selected = selected_index.unwrap_or(0);
    let first_visible = selected.saturating_sub(visible_count.saturating_sub(1));
    let selected_visible = selected_index.map(|index| index.saturating_sub(first_visible));
    let row_width = result_row_width(area, selected_index.is_some());
    let items = results
        .into_iter()
        .skip(first_visible)
        .take(visible_count)
        .map(|row| ListItem::new(result_line(row, row_width)))
        .collect::<Vec<_>>();
    let title = format!(" results ({total}) ");
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(Color::DarkGray))
                .title(Span::styled(title, Style::new().fg(Color::Gray))),
        )
        .highlight_symbol(RESULT_HIGHLIGHT_SYMBOL)
        .highlight_style(
            Style::new()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default();
    state.select(selected_visible);
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_preview(frame: &mut Frame<'_>, area: Rect, preview: &Preview) {
    let (text, style) = match preview {
        Preview::Unavailable => ("no preview", Style::new().fg(Color::DarkGray)),
        Preview::Loading => ("loading preview", Style::new().fg(Color::DarkGray)),
        Preview::Ready(output) => (output.as_str(), Style::new()),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::DarkGray))
        .title(Span::styled(" preview ", Style::new().fg(Color::Gray)));

    frame.render_widget(
        Paragraph::new(text.to_string())
            .style(style)
            .block(block)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn result_line(row: ResultRow, max_width: usize) -> Line<'static> {
    let display_haystack = display_result_haystack(&row.haystack);
    let chars = truncate_middle(&display_haystack, max_width);
    let mut spans = Vec::new();
    let mut text = String::new();
    let mut highlighted = None;

    for display_char in chars {
        let is_highlighted = display_char
            .source_index
            .is_some_and(|index| row.match_indices.binary_search(&index).is_ok());

        if highlighted == Some(is_highlighted) {
            text.push(display_char.ch);
            continue;
        }

        if !text.is_empty() {
            spans.push(styled_result_span(std::mem::take(&mut text), highlighted));
        }

        highlighted = Some(is_highlighted);
        text.push(display_char.ch);
    }

    if !text.is_empty() {
        spans.push(styled_result_span(text, highlighted));
    }

    Line::from(spans)
}

fn display_result_haystack(haystack: &str) -> String {
    let mut display = haystack.to_string();
    if display.starts_with(';') {
        display.replace_range(2..3, " ");
    }
    display
}

fn result_row_width(area: Rect, has_selection: bool) -> usize {
    let border_width = 2;
    let highlight_width = if has_selection {
        RESULT_HIGHLIGHT_SYMBOL.len() as u16
    } else {
        0
    };

    area.width.saturating_sub(border_width + highlight_width) as usize
}

fn styled_result_span(text: String, highlighted: Option<bool>) -> Span<'static> {
    if highlighted == Some(true) {
        Span::styled(
            text,
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw(text)
    }
}

struct DisplayChar {
    ch: char,
    source_index: Option<usize>,
}

fn truncate_middle(text: &str, max_width: usize) -> Vec<DisplayChar> {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max_width {
        return chars
            .into_iter()
            .enumerate()
            .map(|(index, ch)| DisplayChar {
                ch,
                source_index: Some(index),
            })
            .collect();
    }

    if max_width <= 3 {
        return chars
            .into_iter()
            .take(max_width)
            .enumerate()
            .map(|(index, ch)| DisplayChar {
                ch,
                source_index: Some(index),
            })
            .collect();
    }

    let prefix_len = text
        .find(' ')
        .map(|delimiter| text[..=delimiter].chars().count())
        .unwrap_or(0)
        .min(max_width.saturating_sub(3));
    let suffix_len = max_width - prefix_len - 3;
    let suffix_start = chars.len() - suffix_len;

    let prefix = chars
        .iter()
        .copied()
        .take(prefix_len)
        .enumerate()
        .map(|(index, ch)| DisplayChar {
            ch,
            source_index: Some(index),
        });
    let ellipsis = "...".chars().map(|ch| DisplayChar {
        ch,
        source_index: None,
    });
    let suffix = chars
        .iter()
        .copied()
        .enumerate()
        .skip(suffix_start)
        .map(|(index, ch)| DisplayChar {
            ch,
            source_index: Some(index),
        });

    prefix.chain(ellipsis).chain(suffix).collect()
}

fn mode_style(mode: InputMode) -> Style {
    match mode {
        InputMode::Search => Style::new().fg(Color::Cyan),
        InputMode::Edit => Style::new().fg(Color::Yellow),
    }
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

    fn draw(&mut self, app: &App) -> io::Result<()> {
        self.terminal.draw(|frame| render(frame, app))?;
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

    fn displayed_text(chars: Vec<DisplayChar>) -> String {
        chars
            .into_iter()
            .map(|display_char| display_char.ch)
            .collect()
    }

    #[test]
    fn long_result_rows_keep_selector_and_end_visible() {
        assert_eq!(
            displayed_text(truncate_middle(";f /home/todor/dev/fzlaunch/flake.nix", 32)),
            ";f ...dor/dev/fzlaunch/flake.nix"
        );
    }

    #[test]
    fn result_row_width_accounts_for_selected_row_marker() {
        assert_eq!(result_row_width(Rect::new(0, 0, 80, 10), true), 76);
        assert_eq!(result_row_width(Rect::new(0, 0, 80, 10), false), 78);
    }

    #[test]
    fn result_line_highlights_matched_characters() {
        let line = result_line(
            ResultRow {
                haystack: ";f//home/me/paper.pdf".to_string(),
                match_indices: vec![12, 13, 14, 15, 16],
            },
            80,
        );

        let highlighted = line
            .spans
            .iter()
            .filter(|span| span.style.fg == Some(Color::Yellow))
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>();

        assert_eq!(highlighted, vec!["paper"]);
    }

    #[test]
    fn result_line_displays_space_delimiter() {
        let line = result_line(
            ResultRow {
                haystack: ";c/bash".to_string(),
                match_indices: Vec::new(),
            },
            80,
        );

        let text = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(text, ";c bash");
    }

    #[test]
    fn enter_with_incomplete_command_continues() {
        let mut app = App::with_sources([]);

        app.update_input(Value::raw("readlink -f {}"));

        assert_eq!(
            handle_key(&mut app, key(KeyCode::Enter)),
            KeyAction::Continue
        );
        assert_eq!(app.state().queue_status(), Some("readlink -f {}".into()));
    }

    #[test]
    fn enter_with_complete_command_quits_with_command() {
        let mut app = App::with_sources([]);

        app.update_input(Value::raw("nvim"));

        assert_eq!(
            handle_key(&mut app, key(KeyCode::Enter)),
            KeyAction::Quit(Some(Value::raw("nvim")))
        );
    }
}
