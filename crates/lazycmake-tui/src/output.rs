const PREVIEW_MAX_CHARS: usize = 200;

const JOB_OK_PREFIX: &str = "::ok::";
const JOB_ERR_PREFIX: &str = "::err::";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcome {
    Success,
    Failed,
}

/// Marker prefix + timestamped status appended to the output buffer when a job ends.
pub fn format_job_footer(outcome: JobOutcome, unicode: bool, detail: &str) -> String {
    let glyph = job_glyph(outcome, unicode);
    let prefix = match outcome {
        JobOutcome::Success => JOB_OK_PREFIX,
        JobOutcome::Failed => JOB_ERR_PREFIX,
    };
    let timestamp = chrono::Local::now().format("%H:%M:%S");
    format!("{prefix}[{timestamp}] {glyph} {detail}")
}

/// Short label for the Output pane title bar.
pub fn job_title_label(outcome: JobOutcome, unicode: bool, detail: &str) -> String {
    let glyph = job_glyph(outcome, unicode);
    match outcome {
        JobOutcome::Success => format!("{glyph} Finished successfully"),
        JobOutcome::Failed => format!("{glyph} {detail}"),
    }
}

pub fn job_glyph(outcome: JobOutcome, unicode: bool) -> &'static str {
    match (outcome, unicode) {
        (JobOutcome::Success, true) => "✓",
        (JobOutcome::Failed, true) => "✗",
        (JobOutcome::Success, false) => "+",
        (JobOutcome::Failed, false) => "x",
    }
}

pub fn output_line_style(line: &str) -> Option<ratatui::style::Style> {
    use ratatui::style::{Color, Modifier, Style};
    if line.starts_with(JOB_OK_PREFIX) {
        return Some(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD));
    }
    if line.starts_with(JOB_ERR_PREFIX) {
        return Some(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
    }
    None
}

pub fn output_line_display(line: &str) -> &str {
    line.strip_prefix(JOB_OK_PREFIX)
        .or_else(|| line.strip_prefix(JOB_ERR_PREFIX))
        .unwrap_or(line)
}

/// Normalize a subprocess line for TUI display.
///
/// CMake/CTest often emit:
/// - in-place `\r` progress updates
/// - ANSI color / cursor sequences
///
/// Escape bytes must never reach the terminal through ratatui cells — the
/// terminal would reinterpret them and corrupt the alternate screen.
pub fn sanitize_line(raw: &str) -> Option<String> {
    let segment = raw.rsplit('\r').next().unwrap_or(raw);
    let cleaned = strip_controls_and_ansi(segment);
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

pub fn truncate_for_preview(line: &str) -> String {
    if line.chars().count() <= PREVIEW_MAX_CHARS {
        line.to_string()
    } else {
        format!(
            "{}…",
            line.chars().take(PREVIEW_MAX_CHARS).collect::<String>()
        )
    }
}

/// Soft-wrap a single logical line into display rows of at most `width` chars.
pub fn wrap_to_width(line: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars
        .chunks(width)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

fn wrap_line_to_widgets(line: &str, width: usize) -> Vec<ratatui::text::Line<'static>> {
    use ratatui::text::{Line, Span};
    let style = output_line_style(line);
    let display = output_line_display(line);
    wrap_to_width(display, width)
        .into_iter()
        .map(|chunk| {
            if let Some(style) = style {
                Line::from(Span::styled(chunk, style))
            } else {
                Line::from(chunk)
            }
        })
        .collect()
}

/// Build visible output rows for a viewport, preserving job-result styling.
///
/// When `follow` is true, fills from the **end** of the buffer so the newest
/// content stays on screen even if soft-wrapping expands long lines.
pub fn visible_output_widgets(
    lines: &[String],
    width: usize,
    height: usize,
    follow: bool,
    scroll: usize,
) -> Vec<ratatui::text::Line<'static>> {
    if height == 0 || lines.is_empty() {
        return Vec::new();
    }
    let width = width.max(1);

    if follow {
        let mut rows_rev: Vec<ratatui::text::Line<'static>> = Vec::new();
        for line in lines.iter().rev() {
            let mut wrapped = wrap_line_to_widgets(line, width);
            while let Some(row) = wrapped.pop() {
                rows_rev.push(row);
                if rows_rev.len() >= height {
                    rows_rev.reverse();
                    return rows_rev;
                }
            }
        }
        rows_rev.reverse();
        return rows_rev;
    }

    let mut rows: Vec<ratatui::text::Line<'static>> = Vec::new();
    for line in lines.iter().skip(scroll) {
        for row in wrap_line_to_widgets(line, width) {
            rows.push(row);
            if rows.len() >= height {
                return rows;
            }
        }
    }
    rows
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputScroll {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

/// Apply a scroll action to the shared output viewport state.
pub fn apply_output_scroll(
    scroll: &mut usize,
    follow: &mut bool,
    line_count: usize,
    viewport: usize,
    action: OutputScroll,
) {
    if line_count == 0 {
        return;
    }
    let viewport = viewport.max(1);
    let max_start = line_count.saturating_sub(viewport);
    match action {
        OutputScroll::Up => {
            *follow = false;
            *scroll = scroll.saturating_sub(1);
        }
        OutputScroll::Down => {
            *follow = false;
            *scroll = (*scroll + 1).min(max_start);
        }
        OutputScroll::PageUp => {
            *follow = false;
            *scroll = scroll.saturating_sub(viewport);
        }
        OutputScroll::PageDown => {
            *follow = false;
            *scroll = (*scroll + viewport).min(max_start);
        }
        OutputScroll::Home => {
            *follow = false;
            *scroll = 0;
        }
        OutputScroll::End => {
            *follow = true;
            *scroll = max_start;
        }
    }
}

/// Leaving fullscreen returns the main Output pane to live tailing.
pub fn leave_fullscreen_output(follow: &mut bool) {
    *follow = true;
}

fn strip_controls_and_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for c2 in chars.by_ref() {
                        if c2.is_ascii_alphabetic() || c2 == '@' {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    for c2 in chars.by_ref() {
                        if c2 == '\x07' || c2 == '\\' {
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else if c == '\x08' {
            // drop backspace
        } else if !c.is_control() || c == '\t' {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;

    fn widget_text_rows(
        lines: &[String],
        width: usize,
        height: usize,
        follow: bool,
        scroll: usize,
    ) -> Vec<String> {
        visible_output_widgets(lines, width, height, follow, scroll)
            .into_iter()
            .map(|line: Line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn keeps_last_carriage_return_segment() {
        assert_eq!(sanitize_line("old\rnew").as_deref(), Some("new"));
    }

    #[test]
    fn strips_ansi_and_skips_empty() {
        assert_eq!(sanitize_line("\x1b[31mred\x1b[0m").as_deref(), Some("red"));
        assert_eq!(sanitize_line("   ").as_deref(), None);
    }

    #[test]
    fn job_footer_renders_with_status_color() {
        let ok = format_job_footer(JobOutcome::Success, true, "Finished successfully");
        assert!(ok.starts_with(JOB_OK_PREFIX));
        assert!(output_line_style(&ok).is_some());
        assert!(output_line_display(&ok).contains("✓ Finished successfully"));
        assert!(output_line_display(&ok).contains('[') && output_line_display(&ok).contains(']'));

        let err = format_job_footer(JobOutcome::Failed, true, "Failed (exit 2)");
        assert!(output_line_style(&err).is_some());
        assert!(output_line_display(&err).contains("✗ Failed (exit 2)"));
    }

    #[test]
    fn job_title_label_uses_short_success_text() {
        assert_eq!(
            job_title_label(JobOutcome::Success, true, "ignored"),
            "✓ Finished successfully"
        );
        assert_eq!(
            job_title_label(JobOutcome::Failed, true, "Failed (exit 2)"),
            "✗ Failed (exit 2)"
        );
    }

    #[test]
    fn strips_cursor_and_osc_sequences() {
        assert_eq!(
            sanitize_line("\x1b[2J\x1b[Hhello\x1b]0;title\x07 world").as_deref(),
            Some("hello world")
        );
        assert_eq!(sanitize_line("a\x08b").as_deref(), Some("ab"));
    }

    #[test]
    fn truncates_long_preview_lines() {
        let long = "a".repeat(250);
        assert_eq!(truncate_for_preview(&long).chars().count(), 201);
    }

    #[test]
    fn home_and_end_jump_like_vim_g_keys() {
        let mut scroll = 40;
        let mut follow = true;
        apply_output_scroll(&mut scroll, &mut follow, 100, 10, OutputScroll::Home);
        assert!(!follow);
        assert_eq!(scroll, 0);

        apply_output_scroll(&mut scroll, &mut follow, 100, 10, OutputScroll::End);
        assert!(follow);
        assert_eq!(scroll, 90);
    }

    #[test]
    fn leaving_fullscreen_resumes_follow_for_main_pane() {
        let mut follow = false;
        leave_fullscreen_output(&mut follow);
        assert!(follow);
    }

    #[test]
    fn follow_keeps_newest_rows_when_lines_wrap() {
        // Mimic fullscreen: tall viewport of logical lines that wrap to more
        // rows than the height — the error at the end must still appear.
        let lines = vec![
            "noise".into(),
            "x".repeat(30), // wraps to 3 rows at width 10
            "x".repeat(30),
            "CMake Error: Ninja not found".into(),
        ];
        let rows = widget_text_rows(&lines, 10, 4, true, 0);
        assert_eq!(rows.len(), 4);
        let joined = rows.join("");
        assert!(
            joined.contains("CMake Error") && joined.contains("Ninja not found"),
            "expected error in visible rows, got {rows:?}"
        );
        // Last visible content comes from the final logical line, not earlier noise.
        assert!(!joined.contains("noise"));
    }

    #[test]
    fn scroll_mode_starts_from_scroll_line() {
        let lines = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let rows = widget_text_rows(&lines, 80, 2, false, 2);
        assert_eq!(rows, vec!["c".to_string(), "d".to_string()]);
    }
}
