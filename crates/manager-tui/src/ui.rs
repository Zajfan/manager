//! Drawing the screen. Reads the [`App`] state, never changes what it means.

use manager_core::{EntryKind, SortKey, SortOrder};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Cell, Clear, Paragraph, Row as TableRow, Table};

use crate::app::{App, Dialog, Panel, Row, Status};
use crate::format;

/// Below this width the date column is dropped (phones in Termux, split terminals).
const NARROW: u16 = 44;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [panels, status, keys] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(panels);

    let active = app.active;
    draw_panel(frame, left, &mut app.panels[0], active == 0);
    draw_panel(frame, right, &mut app.panels[1], active == 1);
    draw_status(frame, status, app);
    draw_keys(frame, keys);

    if let Some(Dialog::MkDir { input }) = &app.dialog {
        draw_mkdir(frame, input);
    }
}

fn draw_panel(frame: &mut Frame, area: Rect, panel: &mut Panel, is_active: bool) {
    // 2 border lines + 1 header line.
    panel.page_height = usize::from(area.height.saturating_sub(3)).max(1);
    // The rows borrow the panel while we draw, so the scroll state is moved out and back.
    let mut state = std::mem::take(&mut panel.table_state);
    state.select(Some(panel.cursor));
    let border = if is_active {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let loading = if panel.loading.is_some() { " …" } else { "" };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border))
        .title(Line::from(format!(" {}{loading} ", panel.path)).bold())
        .title_bottom(Line::from(summary(panel)).right_aligned());

    let inner_width = area.width.saturating_sub(2);
    let wide = inner_width >= NARROW;
    let mut widths = vec![
        Constraint::Min(6),
        Constraint::Length(5),
        Constraint::Length(7),
    ];
    let mut headers = vec![
        header("Name", SortKey::Name, panel),
        header("Ext", SortKey::Extension, panel),
        header("Size", SortKey::Size, panel),
    ];
    if wide {
        widths.push(Constraint::Length(16));
        headers.push(header("Date", SortKey::Modified, panel));
    }

    let rows: Vec<TableRow> = panel
        .rows()
        .map(|row| table_row(row, panel, wide))
        .collect();

    let highlight = if is_active {
        Style::new().add_modifier(Modifier::REVERSED)
    } else {
        Style::new()
    };
    let table = Table::new(rows, widths)
        .header(TableRow::new(headers).style(Style::new().fg(Color::Yellow)))
        .row_highlight_style(highlight)
        .column_spacing(1)
        .block(block);

    frame.render_stateful_widget(table, area, &mut state);
    panel.table_state = state;
}

fn header<'a>(title: &'a str, key: SortKey, panel: &Panel) -> Cell<'a> {
    if panel.sort.key != key {
        return Cell::from(title);
    }
    let arrow = match panel.sort.order {
        SortOrder::Ascending => "↑",
        SortOrder::Descending => "↓",
    };
    Cell::from(format!("{title}{arrow}")).bold()
}

fn table_row<'a>(row: Row<'a>, panel: &Panel, wide: bool) -> TableRow<'a> {
    let entry = match row {
        Row::Parent => {
            let mut cells = vec![Cell::from(".."), Cell::from(""), Cell::from("<UP>")];
            if wide {
                cells.push(Cell::from(""));
            }
            return TableRow::new(cells).style(Style::new().fg(Color::Blue).bold());
        }
        Row::Entry(entry) => entry,
    };

    let is_dir = entry.kind.is_dir_like();
    let (name, ext) = if is_dir {
        (format!("[{}]", entry.name), String::new())
    } else {
        (
            entry.stem().to_string(),
            entry.extension().unwrap_or("").to_string(),
        )
    };
    let size = match entry.kind {
        _ if is_dir => "<DIR>".to_string(),
        EntryKind::Symlink { target: None } => "BROKEN".to_string(),
        _ => format::size(entry.size),
    };

    let mut style = match entry.kind {
        _ if is_dir => Style::new().fg(Color::Blue).bold(),
        EntryKind::Symlink { target: None } => Style::new().fg(Color::Red),
        EntryKind::Symlink { .. } => Style::new().fg(Color::Cyan),
        EntryKind::Other => Style::new().fg(Color::Magenta),
        _ => Style::new(),
    };
    if entry.hidden {
        style = style.add_modifier(Modifier::DIM);
    }
    if panel.marked.contains(&entry.name) {
        style = style.fg(Color::Yellow).bold();
    }

    let mut cells = vec![
        Cell::from(name),
        Cell::from(ext),
        Cell::from(Line::from(size).right_aligned()),
    ];
    if wide {
        cells.push(Cell::from(format::date(entry.modified)));
    }
    TableRow::new(cells).style(style)
}

/// `3 dirs, 12 files, 4.2M` — or what's marked, when something is.
fn summary(panel: &Panel) -> String {
    let entries = panel.entries();
    let marked: Vec<_> = entries
        .iter()
        .filter(|e| panel.marked.contains(&e.name))
        .collect();
    if !marked.is_empty() {
        let bytes: u64 = marked.iter().map(|e| e.size).sum();
        return format!(" {} marked, {} ", marked.len(), format::size(bytes));
    }
    let dirs = entries.iter().filter(|e| e.kind.is_dir_like()).count();
    let files = entries.len() - dirs;
    let bytes: u64 = entries.iter().map(|e| e.size).sum();
    format!(
        " {}, {}, {} ",
        plural(dirs, "dir"),
        plural(files, "file"),
        format::size(bytes)
    )
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let line = match &app.status {
        Some(Status::Error(msg)) => Line::from(format!(" {msg}")).fg(Color::Red),
        Some(Status::Info(msg)) => Line::from(format!(" {msg}")).fg(Color::Green),
        None => match app.active_panel().current() {
            Some(Row::Entry(entry)) => {
                let target = entry
                    .link_target
                    .as_ref()
                    .map(|t| format!(" -> {}", t.display()))
                    .unwrap_or_default();
                let mode = entry
                    .permissions
                    .unix_mode
                    .map(|m| format!("  {m:o}"))
                    .unwrap_or_default();
                Line::from(format!(" {}{target}{mode}", entry.path)).fg(Color::Gray)
            }
            _ => Line::from(""),
        },
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// The bottom function-key bar. Keys that don't work yet are dimmed.
fn draw_keys(frame: &mut Frame, area: Rect) {
    const KEYS: [(&str, &str, bool); 7] = [
        ("F3", "View", false),
        ("F4", "Edit", false),
        ("F5", "Copy", false),
        ("F6", "Move", false),
        ("F7", "MkDir", true),
        ("F8", "Delete", false),
        ("F10", "Quit", true),
    ];
    let mut spans = Vec::new();
    for (key, label, works) in KEYS {
        let label_style = if works {
            Style::new().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::new().fg(Color::DarkGray).bg(Color::Black)
        };
        spans.push(Span::raw(format!(" {key}")).bold());
        spans.push(Span::styled(format!("{label:<6}"), label_style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_mkdir(frame: &mut Frame, input: &str) {
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(60);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + area.height.saturating_sub(3) / 2,
        width,
        height: 3.min(area.height),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Yellow))
        .title(" New folder ")
        .title_bottom(Line::from(" Enter create · Esc cancel ").right_aligned());
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(format!("{input}█")).block(block), popup);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::loaded_app;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content
            .chunks(usize::from(width))
            .map(|line| line.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn shows_both_panels_with_paths_and_files() {
        let mut app = loaded_app();
        let screen = render(&mut app, 120, 20);
        assert!(screen.contains("sftp://test/home"), "{screen}");
        assert!(screen.contains("sftp://test/tmp"), "{screen}");
        assert!(screen.contains("[docs]"), "{screen}");
        assert!(screen.contains("<DIR>"), "{screen}");
        assert!(screen.contains("1 dir, 2 files"), "{screen}");
        assert!(screen.contains("F7"), "{screen}");
    }

    #[test]
    fn narrow_terminal_drops_date_column_without_crashing() {
        let mut app = loaded_app();
        let screen = render(&mut app, 50, 10);
        assert!(!screen.contains("Date"), "{screen}");
        assert!(screen.contains("Size"), "{screen}");
        // Absurdly small terminals must not panic either.
        render(&mut app, 3, 2);
    }

    #[test]
    fn mkdir_dialog_is_drawn() {
        let mut app = loaded_app();
        app.handle_key(KeyEvent::new(KeyCode::F(7), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        let screen = render(&mut app, 80, 20);
        assert!(screen.contains("New folder"), "{screen}");
        assert!(screen.contains("x█"), "{screen}");
    }
}
