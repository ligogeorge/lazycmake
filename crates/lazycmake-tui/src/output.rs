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
}
