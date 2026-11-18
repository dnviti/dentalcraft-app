//! The app's log files (`dentalcraft.log.<date>` in the log folder): the
//! newest one and its last lines, for the reports and the support bundle.

use std::path::{Path, PathBuf};

/// File name prefix of the rolling log files.
pub const LOG_PREFIX: &str = "dentalcraft.log";

/// The newest log file in `log_dir`, if any.
pub fn newest_log(log_dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(log_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(LOG_PREFIX))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

/// The last `n` lines of the newest log file (empty when there is none).
pub fn last_lines(log_dir: &Path, n: usize) -> Vec<String> {
    let Some(path) = newest_log(log_dir) else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_lines_of_newest_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(last_lines(dir.path(), 5).is_empty());
        let text: String = (0..10).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.path().join(format!("{LOG_PREFIX}.2026-10-31")), text).unwrap();
        std::fs::write(dir.path().join("other.txt"), "x").unwrap();
        assert_eq!(last_lines(dir.path(), 2), ["line 8", "line 9"]);
    }
}
