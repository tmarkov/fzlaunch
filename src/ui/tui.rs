use std::io;
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

use crate::app::Governor;
use crate::model::Value;
use crate::state::{InputMode, ResultRow};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(30);
const RESULT_HIGHLIGHT_SYMBOL: &str = "> ";

pub async fn run(governor: &mut Governor) -> io::Result<Option<Value>> {
    let mut terminal = TerminalSession::enter()?;
    governor.refresh_preview();
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
            governor.refresh_preview();
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
    let results = governor.results();
    let selected_index = governor.selected_index();

    let area = frame.area();
    let shell = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::DarkGray))
        .title(Line::from(vec![
            Span::styled(" fzlaunch ", Style::new().add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {mode} "), mode_style(governor.mode())),
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

    render_input(frame, chunks[0], input.editable_text, governor.mode());
    render_queue(frame, chunks[1], queue);
    render_result_area(
        frame,
        chunks[2],
        results,
        selected_index,
        governor.preview_output(),
    );
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
    preview_output: &str,
) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(2, 3), Constraint::Ratio(1, 3)])
        .split(area);

    render_results(frame, chunks[0], results, selected_index);
    render_preview(frame, chunks[1], preview_output);
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

fn render_preview(frame: &mut Frame<'_>, area: Rect, preview_output: &str) {
    let style = if preview_output == "no preview" {
        Style::new().fg(Color::DarkGray)
    } else {
        Style::new()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::DarkGray))
        .title(Span::styled(" preview ", Style::new().fg(Color::Gray)));

    frame.render_widget(
        Paragraph::new(preview_output.to_string())
            .style(style)
            .block(block)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn result_line(row: ResultRow, max_width: usize) -> Line<'static> {
    let chars = truncate_middle(&row.haystack, max_width);
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
        .map(|space| text[..=space].chars().count())
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
                haystack: ";f /home/me/paper.pdf".to_string(),
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
