use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::Result;

const TAIL_BYTES: u64 = 64 * 1024;

pub fn latest_output_path(log_path: &Path) -> Result<Option<String>> {
    let file = fs::File::open(log_path)?;
    let len = file.metadata()?.len();
    let mut handle = file;
    let start = len.saturating_sub(TAIL_BYTES);
    handle.seek(SeekFrom::Start(start))?;
    let mut buf = String::new();
    handle.read_to_string(&mut buf)?;

    let mut latest: Option<(u64, String)> = None;
    for line in buf.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 4 {
            continue;
        }
        let end = parts[1].parse::<u64>().unwrap_or(0);
        let output = parts[3].to_string();
        if latest.as_ref().is_none_or(|(e, _)| end >= *e) {
            latest = Some((end, output));
        }
    }
    Ok(latest.map(|(_, p)| p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn tail_reads_latest_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".ninja_log");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "# ninja log v5").unwrap();
        writeln!(f, "1\t2\t0\tbuild/old\to1").unwrap();
        writeln!(f, "3\t9\t0\tbuild/new\to2").unwrap();
        let latest = latest_output_path(&path).unwrap();
        assert_eq!(latest.as_deref(), Some("build/new"));
    }
}
