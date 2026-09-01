//! Spawn helpers that keep job I/O off the TUI's terminal.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn apply_job_env(command: &mut Command) {
    command
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0")
        .env("CLICOLOR_FORCE", "0")
        .env("CMAKE_COLOR_DIAGNOSTICS", "OFF")
        .env("CMAKE_COLOR_MAKEFILE", "OFF")
        .env("TERM", "dumb");
}

fn emit_bytes(pending: &mut Vec<u8>, bytes: &[u8], on_line: &mut impl FnMut(String)) {
    for &b in bytes {
        if b == b'\n' || b == b'\r' {
            if !pending.is_empty() {
                let line = String::from_utf8_lossy(pending).into_owned();
                pending.clear();
                on_line(line);
            }
        } else {
            pending.push(b);
            if pending.len() >= 16_384 {
                let line = String::from_utf8_lossy(pending).into_owned();
                pending.clear();
                on_line(line);
            }
        }
    }
}

/// Run `args` with stdout/stderr redirected to a private temp file (never the
/// TUI tty). Poll the file for live lines. On Unix, `setsid` so nested tools
/// cannot open `/dev/tty` and paint over the alternate screen.
pub fn run_job_captured<F>(
    args: &[String],
    cwd: &Path,
    extra_env: &[(String, String)],
    mut on_line: F,
) -> std::io::Result<i32>
where
    F: FnMut(String),
{
    let log = tempfile::NamedTempFile::new()?;
    let log_path: PathBuf = log.path().to_path_buf();

    // One shared fd (cloned) so stdout/stderr append to the same offset.
    // Separate opens without O_APPEND race and overwrite each other.
    let stdout_file = OpenOptions::new().write(true).open(&log_path)?;
    let stderr_file = stdout_file.try_clone()?;

    let mut command = Command::new(&args[0]);
    command
        .args(&args[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    apply_job_env(&mut command);
    for (key, value) in extra_env {
        command.env(key, value);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                // New session: no controlling terminal ⇒ open("/dev/tty") fails.
                libc::setsid();
                Ok(())
            });
        }
    }

    let mut child: Child = command.spawn()?;
    let mut reader = File::open(&log_path)?;
    let mut pending = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut offset: u64 = 0;

    loop {
        reader.seek(SeekFrom::Start(offset))?;
        loop {
            let n = reader.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            offset += n as u64;
            emit_bytes(&mut pending, &chunk[..n], &mut on_line);
        }

        match child.try_wait()? {
            Some(status) => {
                reader.seek(SeekFrom::Start(offset))?;
                loop {
                    let n = reader.read(&mut chunk)?;
                    if n == 0 {
                        break;
                    }
                    emit_bytes(&mut pending, &chunk[..n], &mut on_line);
                }
                if !pending.is_empty() {
                    on_line(String::from_utf8_lossy(&pending).into_owned());
                }
                drop(log);
                return Ok(status.code().unwrap_or(1));
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn shell_quote(arg: &str) -> String {
        if arg.is_empty() {
            return "''".into();
        }
        if arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@+".contains(c))
        {
            return arg.to_string();
        }
        format!("'{}'", arg.replace('\'', "'\\''"))
    }

    fn shell_join(args: &[String]) -> String {
        args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn quotes_shell_metacharacters() {
        assert_eq!(shell_quote("simple"), "simple");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(
            shell_join(&["cmake".into(), "--preset".into(), "tests".into()]),
            "cmake --preset tests"
        );
    }

    #[test]
    fn captures_stdout_without_tty() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let lines_c = lines.clone();
        let code = run_job_captured(
            &["printf".into(), "line1\\nline2\\n".into()],
            Path::new("."),
            &[],
            move |line| lines_c.lock().unwrap().push(line),
        )
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(*lines.lock().unwrap(), vec!["line1", "line2"]);
    }

    #[test]
    fn captures_stderr_and_nonzero_exit() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let lines_c = lines.clone();
        let code = run_job_captured(
            &[
                "sh".into(),
                "-c".into(),
                "echo out_line; echo err_line >&2; exit 7".into(),
            ],
            Path::new("."),
            &[],
            move |line| lines_c.lock().unwrap().push(line),
        )
        .unwrap();
        assert_eq!(code, 7);
        let captured = lines.lock().unwrap().clone();
        assert!(captured.iter().any(|l| l == "out_line"), "{captured:?}");
        assert!(captured.iter().any(|l| l == "err_line"), "{captured:?}");
    }

    #[test]
    fn flush_trailing_partial_line_without_newline() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let lines_c = lines.clone();
        let code = run_job_captured(
            &["printf".into(), "no-newline".into()],
            Path::new("."),
            &[],
            move |line| lines_c.lock().unwrap().push(line),
        )
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(*lines.lock().unwrap(), vec!["no-newline"]);
    }
}
