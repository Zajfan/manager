//! Drawing the screen. Reads the [`App`] state, never changes what it means.

use manager_core::jobs::{JobId, Phase};
use manager_core::{Entry, EntryKind, SortKey, SortOrder, VPath};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Cell, Clear, Paragraph, Row as TableRow, Table};

use crate::app::{App, Dialog, Focus, JobView, Panel, Question, Row, Status};
use crate::compare::Compare;
use crate::format;
use crate::results::Results;
use crate::viewer::{Mode, Viewer};

/// Below this width the date column is dropped (phones in Termux, split terminals).
const NARROW: u16 = 44;

pub fn draw(frame: &mut Frame, app: &mut App) {
    // A folder comparison's results, a search's results, and the viewer all
    // cover everything: in each case you're looking at something other than
    // the two folders.
    if let Some(compare) = app.compare.as_mut() {
        draw_compare(frame, compare);
        return;
    }
    if let Some(results) = app.results.as_mut() {
        draw_results(frame, results);
        return;
    }
    if let Some(viewer) = app.viewer.as_mut() {
        draw_viewer(frame, viewer);
        return;
    }

    // The jobs box only appears while something is running.
    let jobs_height = if app.jobs.is_empty() {
        0
    } else {
        app.jobs.len().min(4) as u16 + 2
    };
    let [panels, jobs, status, keys] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(jobs_height),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(panels);

    let active = app.active;
    let panels_focused = app.focus == Focus::Panels;
    draw_panel(
        frame,
        left,
        &mut app.panels[0],
        panels_focused && active == 0,
    );
    draw_panel(
        frame,
        right,
        &mut app.panels[1],
        panels_focused && active == 1,
    );
    if !app.jobs.is_empty() {
        draw_jobs(frame, jobs, app);
    }
    draw_status(frame, status, app);
    draw_keys(frame, keys);

    // At most one popup: an open dialog first, then the oldest question.
    if let Some(dialog) = &app.dialog {
        draw_dialog(frame, dialog);
    } else if let Some(question) = app.questions.front() {
        let title = app
            .jobs
            .iter()
            .find(|j| Some(j.id) == question_job(question))
            .map(|j| j.title.as_str())
            .unwrap_or_default();
        draw_question(frame, question, title);
    }
}

/// The full-screen list of what a search found.
fn draw_results(frame: &mut Frame, results: &mut Results) {
    let [title, body, status, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    results.fit(usize::from(body.height).max(1));
    let bar = Style::new().bg(Color::Blue).fg(Color::White);
    frame.render_widget(
        Paragraph::new(format!(" Find: {}", results.summary)).style(bar),
        title,
    );

    let selected = results.selected_row();
    let lines: Vec<Line> = if results.hits.is_empty() {
        vec![Line::from(
            if results.running {
                "Searching..."
            } else {
                "Nothing found"
            }
            .dim(),
        )]
    } else {
        results
            .visible()
            .iter()
            .enumerate()
            .map(|(row, entry)| {
                // The folder matters as much as the name in a list of hits.
                let where_ = entry
                    .path
                    .parent()
                    .map(|p| p.to_string())
                    .unwrap_or_default();
                let line = Line::from(vec![
                    Span::raw(entry.name.clone()).bold(),
                    Span::raw("  "),
                    Span::raw(where_).dim(),
                ]);
                if row == selected {
                    line.style(Style::new().bg(Color::Blue).fg(Color::White))
                } else {
                    line
                }
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(lines), body);
    frame.render_widget(Paragraph::new(results.status()), status);
    frame.render_widget(
        Paragraph::new("Enter go to file   ↑↓ PgUp PgDn Home End   EscClose").style(bar),
        keys,
    );
}

/// The full-screen list of what a folder comparison found.
fn draw_compare(frame: &mut Frame, compare: &mut Compare) {
    let [title, body, status, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    compare.fit(usize::from(body.height).max(1));
    let bar = Style::new().bg(Color::Blue).fg(Color::White);
    frame.render_widget(
        Paragraph::new(format!(" Compare: {}", compare.summary)).style(bar),
        title,
    );

    let selected = compare.selected_row();
    let lines: Vec<Line> = if compare.entries.is_empty() {
        vec![Line::from(
            if compare.running {
                "Comparing..."
            } else {
                "No differences"
            }
            .dim(),
        )]
    } else {
        compare
            .visible()
            .iter()
            .enumerate()
            .map(|(row, entry)| compare_line(compare, row, entry, row == selected))
            .collect()
    };
    frame.render_widget(Paragraph::new(lines), body);
    frame.render_widget(Paragraph::new(compare.status()), status);
    frame.render_widget(
        Paragraph::new("Space mark  A all  > L→R  < R→L  U newer  Esc close").style(bar),
        keys,
    );
}

/// One row of the compare view: a mark, a status letter, the name and where
/// it sits, coloured by what kind of difference it is.
fn compare_line(
    compare: &Compare,
    visible_row: usize,
    entry: &manager_core::compare::DiffEntry,
    is_selected: bool,
) -> Line<'static> {
    use manager_core::compare::DiffStatus;

    // `visible_row` is an index into what's on screen; the mark state lives
    // on the underlying (scrolled) index, which `Compare` tracks for us.
    let absolute = compare.first_visible_index() + visible_row;
    let mark = if compare.is_marked(absolute) {
        "*"
    } else {
        " "
    };
    let (letter, color) = match entry.status {
        DiffStatus::LeftOnly => ("<", Color::Green),
        DiffStatus::RightOnly => (">", Color::Green),
        DiffStatus::Differs => ("!", Color::Yellow),
        DiffStatus::KindMismatch => ("?", Color::Red),
        DiffStatus::Same => ("=", Color::DarkGray),
    };
    let where_ = entry.rel_path[..entry.rel_path.len().saturating_sub(1)].join("/");
    let line = Line::from(vec![
        Span::raw(format!("{mark} ")),
        Span::raw(letter).fg(color).bold(),
        Span::raw(" "),
        Span::raw(entry.name().to_string()).bold(),
        Span::raw("  "),
        Span::raw(where_).dim(),
    ]);
    if is_selected {
        line.style(Style::new().bg(Color::Blue).fg(Color::White))
    } else {
        line
    }
}

/// F3's full-screen view of one file.
fn draw_viewer(frame: &mut Frame, viewer: &mut Viewer) {
    let [title, body, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    viewer.fit(usize::from(body.width), usize::from(body.height).max(1));

    let mode = match viewer.mode {
        Mode::Text(encoding) => format!("{encoding:?}").to_lowercase(),
        Mode::Hex => "hex".into(),
    };
    let mut left = format!(" {}  {}", viewer.name, format::size_with_unit(viewer.size));
    if viewer.is_truncated() {
        // Say it plainly rather than letting someone think they've seen it all.
        left.push_str(&format!(
            "  (showing the first {})",
            format::size_with_unit(super::viewer::LIMIT as u64)
        ));
    }
    let right = format!("{mode}  {}% ", viewer.progress());
    // Two passes over the same line: the name from the left, the position from
    // the right. The second only paints its own text, so the first survives.
    let bar = Style::new().bg(Color::Blue).fg(Color::White);
    frame.render_widget(Paragraph::new(left).style(bar), title);
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), title);

    let lines: Vec<Line> = if viewer.loading {
        vec![Line::from("Reading...".dim())]
    } else if let Some(problem) = &viewer.error {
        vec![Line::from(problem.clone().red())]
    } else if viewer.line_count() == 0 {
        vec![Line::from("(empty file)".dim())]
    } else {
        viewer
            .visible()
            .iter()
            .map(|line| Line::from(line.clone()))
            .collect()
    };
    frame.render_widget(Paragraph::new(lines), body);

    let hints = match viewer.mode {
        Mode::Text(_) => "F4Hex   WWrap   ↑↓ PgUp PgDn Home End   EscClose",
        Mode::Hex => "F4Text   ↑↓ PgUp PgDn Home End   EscClose",
    };
    frame.render_widget(
        Paragraph::new(hints).style(Style::new().bg(Color::Blue).fg(Color::White)),
        keys,
    );
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
        format::count(dirs as u64, "dir"),
        format::count(files as u64, "file"),
        format::size(bytes)
    )
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
        ("F5", "Copy", true),
        ("F6", "Move", true),
        ("F7", "MkDir", true),
        ("F8", "Trash", true),
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

fn draw_jobs(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Jobs;
    let hint = if focused {
        " ↑↓ select · P pause/resume · C cancel · Esc back "
    } else {
        " Ctrl+J manage "
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused {
            Color::Cyan
        } else {
            Color::DarkGray
        }))
        .title(Line::from(" Jobs ").bold())
        .title_bottom(Line::from(hint).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Keep the selected job visible when there are more jobs than lines.
    let visible = usize::from(inner.height).max(1);
    let first = app.job_cursor.saturating_sub(visible - 1);
    let lines: Vec<Line> = app
        .jobs
        .iter()
        .enumerate()
        .skip(first)
        .take(visible)
        .map(|(i, job)| {
            let line = job_line(job, inner.width);
            if focused && i == app.job_cursor {
                line.reversed()
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// `Copy 3 items → /tmp  ████████░░░░  62%  45M/s  video.mp4`
fn job_line(job: &JobView, width: u16) -> Line<'static> {
    let p = &job.progress;
    let bar_width = if width > 90 { 20 } else { 10 };
    let filled = (p.fraction() * bar_width as f64).round() as usize;
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(bar_width - filled));

    let state = if p.paused {
        Span::raw(" paused ").fg(Color::Black).bg(Color::Yellow)
    } else {
        match p.phase {
            Phase::Scanning => {
                Span::raw(format!(" counting… {} items ", p.total_items)).fg(Color::Gray)
            }
            _ => Span::raw(format!(" {}/s ", format::size(p.bytes_per_second() as u64))),
        }
    };
    let current = p
        .current
        .as_ref()
        .and_then(VPath::file_name)
        .unwrap_or_default();

    Line::from(vec![
        Span::raw(format!("{} ", job.title)).bold(),
        Span::raw(bar).fg(Color::Cyan),
        Span::raw(format!(" {:>3.0}%", p.fraction() * 100.0)),
        state,
        Span::raw(current).fg(Color::Gray),
    ])
}

fn question_job(question: &Question) -> Option<JobId> {
    match question {
        Question::Conflict(q) => Some(q.job),
        Question::Error(q) => Some(q.job),
    }
}

fn draw_dialog(frame: &mut Frame, dialog: &Dialog) {
    match dialog {
        Dialog::CompareDirs {
            by_content,
            include_hidden,
        } => {
            let tick = |on: &bool| if *on { "[x]" } else { "[ ]" };
            popup(
                frame,
                " Compare folders ",
                vec![
                    Line::from("Compares the left panel against the right one."),
                    Line::from(""),
                    Line::from(format!(
                        "{} Alt+C check content    {} Alt+H compare hidden",
                        tick(by_content),
                        tick(include_hidden)
                    )),
                ],
                " Enter compare · Esc cancel ",
                Color::Yellow,
            )
        }
        Dialog::Find {
            mask,
            text,
            on_text,
            case_sensitive,
            include_hidden,
        } => {
            let cursor = |mine: bool| if mine { "█" } else { "" };
            let tick = |on: &bool| if *on { "[x]" } else { "[ ]" };
            popup(
                frame,
                " Find files ",
                vec![
                    Line::from(format!("Named:      {mask}{}", cursor(!on_text))),
                    Line::from(format!("Containing: {text}{}", cursor(*on_text))),
                    Line::from(""),
                    Line::from(format!(
                        "{} Alt+C match case    {} Alt+H search hidden",
                        tick(case_sensitive),
                        tick(include_hidden)
                    )),
                ],
                " Tab next field · Enter search · Esc cancel ",
                Color::Yellow,
            )
        }
        Dialog::MkDir { input } => popup(
            frame,
            " New folder ",
            vec![Line::from(format!("{input}█"))],
            " Enter create · Esc cancel ",
            Color::Yellow,
        ),
        Dialog::Transfer {
            is_move,
            sources,
            input,
        } => {
            let verb = if *is_move { "Move" } else { "Copy" };
            let title = format!(" {verb} {} to ", describe(sources));
            popup(
                frame,
                &title,
                vec![Line::from(format!("{input}█"))],
                " Enter start · Esc cancel ",
                Color::Yellow,
            )
        }
        Dialog::Delete { targets, permanent } => {
            let (title, text, color) = if *permanent {
                (
                    " Delete permanently ",
                    format!(
                        "Delete {} forever? This can't be undone.",
                        describe(targets)
                    ),
                    Color::Red,
                )
            } else {
                (
                    " Move to trash ",
                    format!("Move {} to the trash?", describe(targets)),
                    Color::Yellow,
                )
            };
            popup(
                frame,
                title,
                vec![Line::from(text)],
                " Enter/Y yes · Esc/N no ",
                color,
            )
        }
    }
}

fn draw_question(frame: &mut Frame, question: &Question, job_title: &str) {
    match question {
        Question::Conflict(q) => {
            let describe = |label: &str, e: &Entry| {
                let size = if e.kind.is_dir_like() {
                    "<DIR>".to_string()
                } else {
                    format::size(e.size)
                };
                Line::from(format!("{label:<9}{size:>8}  {}", format::date(e.modified)))
            };
            let lines = vec![
                Line::from(job_title.to_string()).fg(Color::Gray),
                Line::from(q.existing.path.to_string()).bold(),
                Line::from("already exists."),
                describe("New:", &q.source),
                describe("Existing:", &q.existing),
                Line::from(""),
                Line::from("[O]verwrite  [U]pdate if older  [S]kip  [R]ename  [C]ancel job"),
                Line::from("Shift+letter: same answer for all remaining conflicts").fg(Color::Gray),
            ];
            popup(frame, " File exists ", lines, "", Color::Yellow);
        }
        Question::Error(q) => {
            let lines = vec![
                Line::from(job_title.to_string()).fg(Color::Gray),
                Line::from(q.error.to_string()).fg(Color::Red),
                Line::from(""),
                Line::from("[R]etry  [S]kip  Skip [A]ll  [C]ancel job"),
            ];
            popup(frame, " Problem ", lines, "", Color::Red);
        }
    }
}

/// `report.pdf` for one item, `3 items` for more.
fn describe(paths: &[VPath]) -> String {
    match paths {
        [one] => one.file_name().unwrap_or_else(|| one.to_string()),
        many => format!("{} items", many.len()),
    }
}

/// A centered box drawn over everything else.
fn popup(frame: &mut Frame, title: &str, lines: Vec<Line>, hint: &str, color: Color) {
    let area = frame.area();
    let longest = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let width = (longest.max(title.len() as u16) + 4)
        .clamp(30, 90)
        .min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(Line::from(title.to_string()).bold())
        .title_bottom(Line::from(hint.to_string()).right_aligned());
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
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
    fn running_jobs_show_progress() {
        use manager_core::jobs::{Phase, Progress};
        let mut app = loaded_app();
        app.on_msg(crate::app::Msg::JobStarted {
            id: 1,
            title: "Copy big.iso → /tmp".into(),
        });
        app.on_msg(crate::app::Msg::JobProgress {
            id: 1,
            progress: Progress {
                phase: Phase::Working,
                total_bytes: 1000,
                done_bytes: 500,
                elapsed: std::time::Duration::from_secs(1),
                ..Default::default()
            },
        });
        let screen = render(&mut app, 120, 20);
        assert!(screen.contains("Jobs"), "{screen}");
        assert!(screen.contains("Copy big.iso → /tmp"), "{screen}");
        assert!(screen.contains("50%"), "{screen}");
        assert!(screen.contains("500/s"), "{screen}");
        assert!(screen.contains("Ctrl+J"), "{screen}");
    }

    #[test]
    fn conflict_question_is_drawn() {
        use crate::app::tests::entry;
        use manager_core::jobs::{ConflictQuestion, JobEvent};
        let mut app = loaded_app();
        let dir = crate::app::tests::vp("/tmp");
        let (q, _answer) = ConflictQuestion::new(
            1,
            entry(&dir, "a.txt", EntryKind::File, 10),
            entry(&dir, "a.txt", EntryKind::File, 20),
        );
        app.on_msg(crate::app::Msg::Job(JobEvent::Conflict(Box::new(q))));
        let screen = render(&mut app, 100, 24);
        assert!(screen.contains("File exists"), "{screen}");
        assert!(screen.contains("[O]verwrite"), "{screen}");
    }

    #[test]
    fn permanent_delete_warns() {
        let mut app = loaded_app();
        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT));
        let screen = render(&mut app, 100, 20);
        assert!(screen.contains("Delete b.txt forever?"), "{screen}");
    }

    #[test]
    fn the_viewer_covers_the_screen_with_the_file() {
        let mut app = loaded_app();
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)); // onto "docs"
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)); // onto "a.txt"
        app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
        app.on_msg(crate::app::Msg::Viewed {
            path: app.viewer.as_ref().unwrap().path.clone(),
            result: Ok(b"first line\nsecond line".to_vec()),
        });

        let screen = render(&mut app, 80, 20);

        assert!(screen.contains("a.txt"), "{screen}");
        assert!(screen.contains("first line"), "{screen}");
        assert!(screen.contains("second line"), "{screen}");
        assert!(screen.contains("EscClose"), "{screen}");
        assert!(!screen.contains("b.txt"), "the panels are hidden: {screen}");
    }

    #[test]
    fn a_file_that_would_not_open_shows_why_on_the_viewer() {
        let mut app = loaded_app();
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
        let path = app.viewer.as_ref().unwrap().path.clone();
        app.on_msg(crate::app::Msg::Viewed {
            path: path.clone(),
            result: Err(manager_core::Error::PermissionDenied(path)),
        });

        let screen = render(&mut app, 80, 20);

        assert!(screen.contains("permission denied"), "{screen}");
    }

    #[test]
    fn the_find_dialog_shows_both_fields_and_the_switches() {
        let mut app = loaded_app();
        app.handle_key(KeyEvent::new(KeyCode::F(7), KeyModifiers::ALT));

        let screen = render(&mut app, 80, 20);

        assert!(screen.contains("Find files"), "{screen}");
        assert!(screen.contains("Named:"), "{screen}");
        assert!(screen.contains("Containing:"), "{screen}");
        assert!(screen.contains("Alt+C match case"), "{screen}");
    }

    #[test]
    fn the_results_list_shows_hits_and_where_they_are() {
        let mut app = loaded_app();
        app.handle_key(KeyEvent::new(KeyCode::F(7), KeyModifiers::ALT));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let found = crate::app::tests::entry(
            &crate::app::tests::vp("/home/docs"),
            "guide.md",
            EntryKind::File,
            10,
        );
        app.on_msg(crate::app::Msg::SearchFound(Box::new(found)));

        let screen = render(&mut app, 80, 20);

        assert!(screen.contains("guide.md"), "{screen}");
        assert!(screen.contains("docs"), "the folder it's in: {screen}");
        assert!(screen.contains("Searching"), "{screen}");
        assert!(!screen.contains("b.txt"), "the panels are hidden: {screen}");
    }

    #[test]
    fn a_search_that_found_nothing_says_so_rather_than_showing_a_blank() {
        let mut app = loaded_app();
        app.handle_key(KeyEvent::new(KeyCode::F(7), KeyModifiers::ALT));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.on_msg(crate::app::Msg::SearchFinished(
            manager_core::search::SearchReport {
                found: 0,
                scanned: 12,
                unreadable: 0,
                cancelled: false,
            },
        ));

        let screen = render(&mut app, 80, 20);

        assert!(screen.contains("Nothing found"), "{screen}");
    }

    #[test]
    fn the_compare_dialog_shows_its_switches() {
        let mut app = loaded_app();
        app.handle_key(KeyEvent::new(KeyCode::F(9), KeyModifiers::CONTROL));

        let screen = render(&mut app, 80, 20);

        assert!(screen.contains("Compare folders"), "{screen}");
        assert!(screen.contains("Alt+C check content"), "{screen}");
    }

    #[test]
    fn the_compare_view_shows_differences_with_a_status_letter() {
        let mut app = loaded_app();
        app.handle_key(KeyEvent::new(KeyCode::F(9), KeyModifiers::CONTROL));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let missing = crate::app::tests::entry(
            &crate::app::tests::vp("/home"),
            "only-left.txt",
            EntryKind::File,
            10,
        );
        app.on_msg(crate::app::Msg::CompareFound(Box::new(
            manager_core::compare::DiffEntry {
                rel_path: vec!["only-left.txt".into()],
                left: Some(missing),
                right: None,
                status: manager_core::compare::DiffStatus::LeftOnly,
                newer: None,
            },
        )));

        let screen = render(&mut app, 80, 20);

        assert!(screen.contains("only-left.txt"), "{screen}");
        assert!(screen.contains("Comparing"), "{screen}");
        assert!(!screen.contains("b.txt"), "the panels are hidden: {screen}");
    }

    #[test]
    fn a_comparison_with_no_differences_says_so_rather_than_showing_a_blank() {
        let mut app = loaded_app();
        app.handle_key(KeyEvent::new(KeyCode::F(9), KeyModifiers::CONTROL));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.on_msg(crate::app::Msg::CompareFinished(
            manager_core::compare::CompareReport {
                differences: 0,
                scanned: 20,
                unreadable: 0,
                cancelled: false,
            },
        ));

        let screen = render(&mut app, 80, 20);

        assert!(screen.contains("No differences"), "{screen}");
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
