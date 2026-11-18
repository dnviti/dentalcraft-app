//! The events waiting to be sent: one JSON file per event in a folder
//! (`<data>/diagnostics/queue`), named so that the oldest sorts first. Files
//! survive a crash or a restart; the folder is bounded, the oldest events
//! go first.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::Event;

/// Most events kept.
pub const MAX_QUEUED: usize = 500;
/// Most bytes kept.
pub const MAX_QUEUED_BYTES: u64 = 5 * 1024 * 1024;

/// A queued event file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub path: PathBuf,
    pub size: u64,
}

/// The folder of the events waiting to be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queue {
    dir: PathBuf,
    max_events: usize,
    max_bytes: u64,
}

impl Queue {
    /// The queue in `dir`, bounded by [`MAX_QUEUED`] events and
    /// [`MAX_QUEUED_BYTES`].
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self::bounded(dir, MAX_QUEUED, MAX_QUEUED_BYTES)
    }

    /// The queue in `dir` with other bounds.
    pub fn bounded(dir: impl Into<PathBuf>, max_events: usize, max_bytes: u64) -> Self {
        Self {
            dir: dir.into(),
            max_events,
            max_bytes,
        }
    }

    /// The folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Adds `event` (written whole or not at all), then drops the oldest
    /// events beyond the bounds.
    pub fn push(&self, event: &Event) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        // the time, then a counter: the events of one run keep their order
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let seq = SEQ.fetch_add(1, Ordering::Relaxed) % 1_000_000;
        let name = format!("{millis:016}-{seq:06}-{}.json", event.id);
        let tmp = self.dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec(event)?)?;
        std::fs::rename(&tmp, self.dir.join(name))?;
        self.trim();
        Ok(())
    }

    /// The event files, oldest first.
    pub(crate) fn entries(&self) -> Vec<Entry> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<Entry> = rd
            .filter_map(Result::ok)
            .filter(|e| {
                let n = e.file_name();
                let n = n.to_string_lossy();
                !n.starts_with('.') && n.ends_with(".json")
            })
            .filter_map(|e| {
                Some(Entry {
                    size: e.metadata().ok()?.len(),
                    path: e.path(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// How many events wait.
    pub fn len(&self) -> usize {
        self.entries().len()
    }

    /// `true` when nothing waits.
    pub fn is_empty(&self) -> bool {
        self.entries().is_empty()
    }

    fn trim(&self) {
        let entries = self.entries();
        let mut total: u64 = entries.iter().map(|e| e.size).sum();
        let mut count = entries.len();
        for e in entries {
            if count <= self.max_events && total <= self.max_bytes {
                break;
            }
            if std::fs::remove_file(&e.path).is_ok() {
                count -= 1;
                total = total.saturating_sub(e.size);
            }
        }
    }

    /// The oldest events, at most `max_events` of them and `max_bytes` of
    /// JSON (at least one when any waits). An unreadable file is deleted.
    pub(crate) fn batch(&self, max_events: usize, max_bytes: usize) -> Vec<(PathBuf, Event)> {
        let mut out = Vec::new();
        let mut bytes = 0;
        for e in self.entries() {
            if out.len() >= max_events {
                break;
            }
            let Ok(data) = std::fs::read(&e.path) else {
                continue;
            };
            let Ok(event) = serde_json::from_slice::<Event>(&data) else {
                let _ = std::fs::remove_file(&e.path);
                continue;
            };
            if !out.is_empty() && bytes + data.len() + 1 > max_bytes {
                break;
            }
            bytes += data.len() + 1;
            out.push((e.path, event));
        }
        out
    }

    /// Deletes the event files `paths`.
    pub(crate) fn remove(&self, paths: &[PathBuf]) {
        for p in paths {
            let _ = std::fs::remove_file(p);
        }
    }

    /// Deletes every waiting event.
    pub fn clear(&self) {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for e in rd.filter_map(Result::ok) {
            if e.path().is_file() {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kind, Source};

    fn event(n: usize, message: &str) -> Event {
        Event {
            id: format!("{n:04}"),
            kind: Kind::Error,
            at: "2026-11-18T15:59:58Z".into(),
            source: Source::App,
            code: "c".into(),
            message: message.into(),
            command: None,
            module: None,
            backtrace: None,
            log_tail: None,
            context: None,
        }
    }

    #[test]
    fn events_survive_in_order_and_go_when_sent() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::new(dir.path().join("queue"));
        assert!(q.is_empty());
        for n in 0..3 {
            q.push(&event(n, "m")).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // another handle on the same folder (the next start) sees them
        let again = Queue::new(dir.path().join("queue"));
        let batch = again.batch(2, 1 << 20);
        assert_eq!(
            batch.iter().map(|(_, e)| e.id.as_str()).collect::<Vec<_>>(),
            ["0000", "0001"]
        );
        again.remove(&batch.into_iter().map(|(p, _)| p).collect::<Vec<_>>());
        assert_eq!(q.len(), 1);
        assert_eq!(q.batch(10, 1 << 20)[0].1.id, "0002");
        // a damaged file is dropped, not sent
        std::fs::write(q.dir().join("0000000000000000-bad.json"), "{").unwrap();
        assert_eq!(q.batch(10, 1 << 20).len(), 1);
        assert_eq!(q.len(), 1);
        q.clear();
        assert!(q.is_empty());
    }

    #[test]
    fn the_oldest_go_beyond_the_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::bounded(dir.path(), 3, 1 << 20);
        for n in 0..5 {
            q.push(&event(n, "m")).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let ids: Vec<String> = q
            .batch(10, 1 << 20)
            .into_iter()
            .map(|(_, e)| e.id)
            .collect();
        assert_eq!(ids, ["0002", "0003", "0004"]);
        // bytes: two big events do not fit in 3000 bytes
        let small = Queue::bounded(dir.path().join("b"), 100, 3000);
        for n in 0..3 {
            small.push(&event(n, &"x".repeat(1200))).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(small.len(), 2);
        // a batch stops before the byte limit, but always takes one
        assert_eq!(small.batch(10, 2000).len(), 1);
        assert_eq!(small.batch(10, 10).len(), 1);
    }
}
