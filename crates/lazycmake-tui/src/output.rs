const PREVIEW_MAX_CHARS: usize = 200;

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

/// Build the visible output rows for a viewport.
///
/// When `follow` is true, fills from the **end** of the buffer so the newest
/// content stays on screen even if soft-wrapping expands long lines. The old
/// fullscreen path took the last N *logical* lines and then wrapped them,
/// which pushed the real tail (errors) below the clip region.
pub fn visible_output_rows(
    lines: &[String],
    width: usize,
    height: usize,
    follow: bool,
    scroll: usize,
) -> Vec<String> {
    if height == 0 || lines.is_empty() {
        return Vec::new();
    }
    let width = width.max(1);

    if follow {
        let mut rows_rev: Vec<String> = Vec::new();
        for line in lines.iter().rev() {
            let mut wrapped = wrap_to_width(line, width);
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

    let mut rows: Vec<String> = Vec::new();
    for line in lines.iter().skip(scroll) {
        for row in wrap_to_width(line, width) {
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
        let rows = visible_output_rows(&lines, 10, 4, true, 0);
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
        let rows = visible_output_rows(&lines, 80, 2, false, 2);
        assert_eq!(rows, vec!["c".to_string(), "d".to_string()]);
    }
}
