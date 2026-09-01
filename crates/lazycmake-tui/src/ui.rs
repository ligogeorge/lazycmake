use lazycmake_core::{FocusedColumn, TargetKind, TestStatus};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, StatefulWidget, Wrap,
};
use ratatui::Frame;

use crate::output::{truncate_for_preview, visible_output_widgets, JobOutcome};
use crate::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Filter,
    Help,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    CleanConfigure,
    CleanBuild,
}

pub struct DrawState<'a> {
    pub app: &'a App,
    pub preset_visible: &'a [usize],
    pub target_visible: &'a [usize],
    pub test_visible: &'a [usize],
}

pub fn draw(frame: &mut Frame, state: DrawState<'_>) {
    if state.app.mode == Mode::Output {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(2)])
            .split(frame.area());
        draw_output_fullscreen(frame, chunks[0], state.app);
        draw_output_status(frame, chunks[1], state.app);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(55),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(frame.area());

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(40),
            Constraint::Percentage(35),
        ])
        .split(chunks[0]);

    draw_preset_column(frame, columns[0], &state);
    draw_target_column(frame, columns[1], &state);
    draw_test_column(frame, columns[2], &state);
    draw_output(frame, chunks[1], state.app);
    if state.app.mode == Mode::Filter {
        draw_filter_bar(frame, chunks[2], state.app);
    } else {
        draw_status(frame, chunks[2], state.app);
    }

    if state.app.mode == Mode::Help {
        draw_help(frame);
    }
    if state.app.confirm.is_some() {
        draw_confirm(frame, state.app.confirm.unwrap());
    }
}

fn padded_title(mut title: Line<'_>) -> Line<'_> {
    title.spans.insert(0, Span::raw(" "));
    title.spans.push(Span::raw(" "));
    title
}

fn bordered_block(title: Line<'_>) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(padded_title(title))
        .padding(Padding::horizontal(1))
}

fn draw_scrollable_list(
    frame: &mut Frame,
    area: Rect,
    title: Line<'_>,
    items: Vec<ListItem>,
    focused: bool,
    selected: usize,
) {
    let total = items.len();
    let pos = if total == 0 { 0 } else { selected + 1 };
    let mut title = title;
    title.spans.push(Span::raw(format!(" [{pos}/{total}]")));
    let block = bordered_block(title);

    let mut list_state = ListState::default();
    if focused && total > 0 {
        list_state.select(Some(selected.min(total - 1)));
    }

    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .add_modifier(Modifier::REVERSED)
            .add_modifier(Modifier::BOLD),
    );
    StatefulWidget::render(list, area, frame.buffer_mut(), &mut list_state);
}

fn draw_preset_column(frame: &mut Frame, area: Rect, state: &DrawState<'_>) {
    let focused = state.app.state.focused_column == FocusedColumn::Presets;
    let visible = state.preset_visible.len();
    let total = state.app.preset_names.len();
    let title = if visible != total {
        Line::from(format!("Presets [{visible}/{total}]"))
    } else {
        Line::from(format!("Presets ({total})"))
    };
    let items: Vec<ListItem> = state
        .preset_visible
        .iter()
        .map(|&idx| {
            let name = state.app.preset_names.get(idx).map(String::as_str).unwrap_or("");
            ListItem::new(Line::from(name))
        })
        .collect();
    draw_scrollable_list(
        frame,
        area,
        title,
        items,
        focused,
        state.app.state.presets.selected,
    );
}

fn draw_target_column(frame: &mut Frame, area: Rect, state: &DrawState<'_>) {
    let focused = state.app.state.focused_column == FocusedColumn::Targets;
    let visible = state.target_visible.len();
    let total = state.app.targets.len();
    let title = if visible != total {
        Line::from(format!("Targets [{visible}/{total}]"))
    } else {
        Line::from(format!("Targets ({total})"))
    };
    let kind_width = 3; // exe/lib/utl/oth
    // borders (2) + horizontal padding (2)
    let inner_width = area.width.saturating_sub(4) as usize;
    let name_width = inner_width.saturating_sub(kind_width + 3).max(8); // " [xxx]"
    let items: Vec<ListItem> = state
        .target_visible
        .iter()
        .map(|&idx| {
            let Some(target) = state.app.targets.get(idx) else {
                return ListItem::new(Line::from(""));
            };
            let name = truncate_pad(&target.name, name_width);
            let kind_style = match target.kind {
                TargetKind::Executable => Style::default().fg(Color::Green),
                TargetKind::Library => Style::default().fg(Color::Cyan),
                TargetKind::Utility => Style::default().fg(Color::Yellow),
                TargetKind::Other => Style::default().fg(Color::DarkGray),
            };
            ListItem::new(Line::from(vec![
                Span::raw(name),
                Span::raw(" ["),
                Span::styled(target.kind.label(), kind_style),
                Span::raw("]"),
            ]))
        })
        .collect();
    draw_scrollable_list(
        frame,
        area,
        title,
        items,
        focused,
        state.app.state.targets.selected,
    );
}

fn draw_test_column(frame: &mut Frame, area: Rect, state: &DrawState<'_>) {
    let focused = state.app.state.focused_column == FocusedColumn::Tests;
    let (pass, fail, skip, unknown) = state.app.tests.counts();
    let unicode = state.app.unicode_glyphs;
    let pass_g = TestStatus::Pass.glyph(unicode);
    let fail_g = TestStatus::Fail.glyph(unicode);
    let skip_g = TestStatus::Skip.glyph(unicode);
    let unknown_g = TestStatus::Unknown.glyph(unicode);
    let title = Line::from(vec![
        Span::raw("Tests  "),
        Span::styled(format!("{pass_g} {pass}"), Style::default().fg(Color::Green)),
        Span::raw("  "),
        Span::styled(format!("{fail_g} {fail}"), Style::default().fg(Color::Red)),
        Span::raw("  "),
        Span::styled(format!("{skip_g} {skip}"), Style::default().fg(Color::Yellow)),
        Span::raw("  "),
        Span::styled(format!("{unknown_g} {unknown}"), Style::default().fg(Color::DarkGray)),
    ]);
    let items: Vec<ListItem> = state
        .test_visible
        .iter()
        .filter_map(|&idx| {
            let case = state.app.tests.cases.get(idx)?;
            let glyph = case.status.glyph(state.app.unicode_glyphs);
            let status_style = match case.status {
                TestStatus::Pass => Style::default().fg(Color::Green),
                TestStatus::Fail => Style::default().fg(Color::Red),
                TestStatus::Skip => Style::default().fg(Color::Yellow),
                TestStatus::Unknown => Style::default().fg(Color::DarkGray),
            };
            let line = Line::from(vec![
                Span::styled(format!("{glyph} "), status_style),
                Span::raw(case.name.as_str()),
            ]);
            Some(ListItem::new(line))
        })
        .collect();
    draw_scrollable_list(
        frame,
        area,
        title,
        items,
        focused,
        state.app.state.tests.selected,
    );
}

fn truncate_pad(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let count = text.chars().count();
    if count > width {
        if width == 1 {
            return "…".into();
        }
        format!("{}…", text.chars().take(width - 1).collect::<String>())
    } else {
        format!("{text:<width$}")
    }
}

fn draw_output(frame: &mut Frame, area: Rect, app: &App) {
    let inner_height = area.height.saturating_sub(2) as usize;
    // borders (2) + horizontal padding (2)
    let inner_width = area.width.saturating_sub(4) as usize;
    let preview: Vec<String> = app
        .output
        .iter()
        .map(|l| truncate_for_preview(l))
        .collect();
    let lines = visible_output_widgets(
        &preview,
        inner_width.max(1),
        inner_height,
        app.output_follow,
        app.output_scroll,
    );
    let follow = if app.output_follow { "follow" } else { "scroll" };
    let block = bordered_block(output_title(
        app,
        &format!("Output [{follow}] — press o for full"),
    ));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_output_fullscreen(frame: &mut Frame, area: Rect, app: &App) {
    let inner_height = area.height.saturating_sub(2) as usize;
    let inner_width = area.width.saturating_sub(4) as usize;
    let lines = visible_output_widgets(
        &app.output,
        inner_width.max(1),
        inner_height,
        app.output_follow,
        app.output_scroll,
    );
    let start = if app.output_follow {
        app.output.len().saturating_sub(inner_height.max(1))
    } else {
        app.output_scroll
            .min(app.output.len().saturating_sub(inner_height.max(1)))
    };
    let pos = if app.output.is_empty() {
        0
    } else {
        start.saturating_add(1)
    };
    let base = format!("Output [{pos}/{} lines]", app.output.len());
    let block = bordered_block(output_title(app, &base));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn output_title(app: &App, base: &str) -> Line<'static> {
    if app.job_running {
        return Line::from(format!("{base}  …"));
    }
    let Some(outcome) = app.last_job_outcome else {
        return Line::from(base.to_string());
    };
    let color = match outcome {
        JobOutcome::Success => Color::Green,
        JobOutcome::Failed => Color::Red,
    };
    Line::from(vec![
        Span::raw(format!("{base}  ")),
        Span::styled(
            app.last_job_summary.clone(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn draw_output_status(frame: &mut Frame, area: Rect, app: &App) {
    let follow = if app.output_follow { "on" } else { "off" };
    let line = Line::from(vec![Span::raw(format!(
        "[o/Esc] back  [↑↓/jk] scroll  [PgUp/PgDn] page  [g/G] top/bottom  [f] follow={follow}  [q] quit"
    ))]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_filter_bar(frame: &mut Frame, area: Rect, app: &App) {
    let column = match app.state.focused_column {
        FocusedColumn::Presets => "Presets",
        FocusedColumn::Targets => "Targets",
        FocusedColumn::Tests => "Tests",
    };
    let line = Line::from(vec![
        Span::styled(format!("Filter {column}: /"), Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(app.filter_input.as_str()),
        Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
        Span::raw("  [Enter] select/build/run  [Esc] cancel"),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let preset = app.selected_preset.as_deref().unwrap_or("-");
    let mut spans = vec![Span::raw(format!("Preset: {preset}"))];
    if !app.job_running && !app.status_message.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            app.status_message.clone(),
            Style::default().fg(Color::Red),
        ));
    }
    spans.push(Span::raw(
        "  [↑↓] Move  [Enter] Act  [c] Configure  [b] Build  [t/T] Test  [o] Output  [?] Help  [q] Quit",
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_help(frame: &mut Frame) {
    let area = centered_rect(68, 78, frame.area());
    frame.render_widget(Clear, area);

    let key = |k: &str, desc: &str| {
        Line::from(vec![
            Span::styled(
                format!("  {k:<18}"),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::raw(desc.to_string()),
        ])
    };

    let section = |title: &'static str| {
        Line::from(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD).fg(Color::Yellow),
        ))
    };

    let text = vec![
        section(" Navigation"),
        key("↑↓ / j k", "move selection in focused column"),
        key("Tab / Shift+Tab", "focus Presets → Targets → Tests"),
        key("PgUp / PgDn", "page up / down in list"),
        key("Home / End", "first / last item in list"),
        key("/", "filter focused column"),
        Line::from(""),
        section(" Presets"),
        key("Enter", "select preset (auto-configure if needed)"),
        key("c", "configure selected preset"),
        key("C", "clean cache, then reconfigure (confirm)"),
        Line::from(""),
        section(" Targets"),
        key("Enter / b", "build selected target"),
        key("B", "clean build, then build target (confirm)"),
        key("r", "run selected executable target"),
        Line::from(""),
        section(" Tests"),
        key("Enter / t", "run selected test"),
        key("T", "run all tests"),
        key("F", "toggle failing-only filter"),
        Line::from(""),
        section(" Output"),
        key("o", "open fullscreen output"),
        key("↑↓ / j k", "scroll (fullscreen)"),
        key("PgUp / PgDn", "page scroll (fullscreen)"),
        key("g / G", "jump to top / bottom (fullscreen)"),
        key("f", "toggle follow mode (fullscreen)"),
        key("Esc / o", "close fullscreen output"),
        Line::from(""),
        section(" Filter (after /)"),
        key("type", "narrow the focused list"),
        key("↑↓", "move selection while filtering"),
        key("Enter", "apply filter and run column action"),
        key("Esc", "cancel filter"),
        Line::from(""),
        section(" General"),
        key("?", "this help"),
        key("Esc", "close help / cancel confirm"),
        key("q", "quit"),
    ];

    let block = bordered_block(Line::from("Help — press Esc to close"))
        .style(Style::default().bg(Color::Black));
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .style(Style::default().bg(Color::Black).fg(Color::White))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_confirm(frame: &mut Frame, action: ConfirmAction) {
    let area = centered_rect(50, 24, frame.area());
    frame.render_widget(Clear, area);
    let label = match action {
        ConfirmAction::CleanConfigure => "Clean cache and reconfigure?",
        ConfirmAction::CleanBuild => "Clean build?",
    };
    let block = bordered_block(Line::from("Confirm")).style(Style::default().bg(Color::Black));
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::raw(format!("  {label}"))),
            Line::from(""),
            Line::from(vec![
                Span::raw("  "),
                Span::styled("y", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw(" yes   "),
                Span::styled("n", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Span::raw(" / Esc  no"),
            ]),
        ])
        .block(block)
        .style(Style::default().bg(Color::Black).fg(Color::White)),
        area,
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
