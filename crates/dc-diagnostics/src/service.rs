//! [`Diagnostics`]: the consent, the queue, the recent log lines, and the
//! worker thread that sends the reports.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::crash::{self, Crash};
use crate::event::ReportBody;
use crate::sanitize::{clean_code, cut, cut_front};
use crate::settings::{Consent, Paths};
use crate::{
    AppInfo, DiagLayer, Event, Gpu, Kind, MAX_BACKTRACE, MAX_BODY, MAX_CONTEXT, MAX_EVENTS,
    MAX_LOG_TAIL, MAX_MESSAGE, NewEvent, Platform, Queue, Sanitizer, SendOutcome, Source,
};

/// Log lines kept in memory.
const TAIL_LINES: usize = 200;
/// Longest kept log line, in bytes.
const TAIL_LINE_MAX: usize = 1000;
/// Log lines attached to an error.
const ERROR_TAIL_LINES: usize = 50;
/// Log lines of the crashed run attached to a crash picked up at start.
const CRASH_TAIL_LINES: usize = 200;
/// The same event again within this time is not recorded again.
const DEDUP_WINDOW: Duration = Duration::from_secs(60);
/// Most error and warning events of one run (a loop of errors does not
/// flood the queue).
const MAX_PER_RUN: usize = 300;
/// New events are sent this long after the last one...
const DEBOUNCE: Duration = Duration::from_secs(5);
/// ...but at most this long after the first one waiting.
const MAX_DELAY: Duration = Duration::from_secs(60);
/// The queue is looked at this often while the app runs.
const PERIOD: Duration = Duration::from_secs(10 * 60);
/// First wait after a failed send; it doubles up to [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(60);
/// Longest wait between failed sends.
const MAX_BACKOFF: Duration = Duration::from_secs(60 * 60);
/// Longest `Retry-After` honoured.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(6 * 60 * 60);
/// Bytes of events in one report (the server takes [`MAX_BODY`] in all).
const BATCH_BYTES: usize = MAX_BODY - 16 * 1024;
/// Most reports sent in one go.
const MAX_BATCHES: usize = 10;
/// File (in the diagnostics folder) with the time of the consent: crash
/// files older than it are not reported.
const SINCE_FILE: &str = "crash-since";
/// File (in the diagnostics folder) with the salt (first line) and the
/// salted digests (one hex per line) of the patient codes and case
/// references registered in every run: crash files and log tails of earlier
/// runs are cleaned of them before anything is queued. The plain codes are
/// never written.
const REDACT_FILE: &str = "redact";

/// Where the diagnostics live and go.
#[derive(Debug, Clone)]
pub struct Config {
    /// The diagnostics folder (`<data>/diagnostics`; the queue is in
    /// `queue/`).
    pub dir: PathBuf,
    /// The log folder (crash files, log lines of a crashed run).
    pub log_dir: PathBuf,
    /// Where the reports go ([`crate::endpoint`]).
    pub endpoint: String,
    /// The program.
    pub app: AppInfo,
    /// The machine.
    pub platform: Platform,
    /// What is removed from the texts.
    pub sanitizer: Sanitizer,
}

impl Config {
    /// The app's folders, this program and machine, reports to the server
    /// `issuer`.
    pub fn standard(paths: &Paths, issuer: &str) -> Self {
        Self {
            dir: paths.diagnostics(),
            log_dir: paths.logs(),
            endpoint: crate::endpoint(issuer),
            app: AppInfo::current(),
            platform: Platform::current(None),
            sanitizer: Sanitizer::current(),
        }
    }
}

/// What a send did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flush {
    /// Events sent and taken by the server.
    pub sent: usize,
    /// Of those, new to the server.
    pub accepted: u64,
    /// Of those, already known to the server.
    pub duplicates: u64,
    /// Events the server refused as invalid or too large (deleted).
    pub dropped: usize,
    /// Events still waiting.
    pub remaining: usize,
    /// Why the send stopped early (no connection, server busy): the events
    /// stay and go later.
    pub retry: Option<String>,
    /// The server's `Retry-After`.
    pub retry_after: Option<Duration>,
}

impl Flush {
    /// The output of `app.diagnostics.send`.
    pub fn to_json(&self) -> Value {
        json!({
            "sent": self.sent,
            "accepted": self.accepted,
            "duplicates": self.duplicates,
            "dropped": self.dropped,
            "remaining": self.remaining,
        })
    }
}

struct Shared {
    install_id: Option<String>,
    endpoint: String,
    app: AppInfo,
    platform: Platform,
    sanitizer: Sanitizer,
    last: Option<(Kind, Source, String, String, Instant)>,
    recorded: usize,
    startup_ms: Option<u64>,
    session_recorded: bool,
}

struct Worker {
    running: bool,
    send_now: bool,
    first_new: Option<Instant>,
    last_new: Option<Instant>,
    retry_at: Option<Instant>,
    backoff: Duration,
    next_periodic: Instant,
}

impl Worker {
    /// When the next send is due.
    fn due(&self, now: Instant) -> Instant {
        let mut when = self.next_periodic;
        if let (Some(first), Some(last)) = (self.first_new, self.last_new) {
            when = when.min((last + DEBOUNCE).min(first + MAX_DELAY));
        }
        if self.send_now {
            when = now;
        }
        match self.retry_at {
            Some(r) => when.max(r),
            None => when,
        }
    }
}

struct Inner {
    enabled: AtomicBool,
    started: Instant,
    queue: Queue,
    dir: PathBuf,
    log_dir: PathBuf,
    shared: Mutex<Shared>,
    tail: Mutex<VecDeque<String>>,
    sending: Mutex<()>,
    worker: Mutex<Worker>,
    wake: Condvar,
}

/// The diagnostics of one program run. Cheap to clone (one shared state).
#[derive(Clone)]
pub struct Diagnostics {
    inner: Arc<Inner>,
}

/// `lock` or, from the panic hook (`wait == false`), `try_lock`: a panic
/// while the lock is held must not hang the program.
fn take<T>(m: &Mutex<T>, wait: bool) -> Option<MutexGuard<'_, T>> {
    if wait {
        return Some(m.lock().unwrap_or_else(PoisonError::into_inner));
    }
    match m.try_lock() {
        Ok(g) => Some(g),
        Err(TryLockError::Poisoned(p)) => Some(p.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn hex32(d: &[u8; 32]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0_u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// The salt and digests of [`REDACT_FILE`] (a new salt when there is none).
fn read_redact(dir: &Path) -> (String, Vec<[u8; 32]>) {
    let text = std::fs::read_to_string(dir.join(REDACT_FILE)).unwrap_or_default();
    let mut lines = text.lines();
    match lines.next().map(str::trim).filter(|s| !s.is_empty()) {
        Some(salt) => (salt.to_owned(), lines.filter_map(parse_hex32).collect()),
        None => (new_id(), Vec::new()),
    }
}

/// Adds `digest` to [`REDACT_FILE`] (written with `salt` when new).
fn append_redact(dir: &Path, salt: &str, digest: &[u8; 32]) {
    use std::io::Write;
    let path = dir.join(REDACT_FILE);
    let new = !path.exists();
    let _ = std::fs::create_dir_all(dir);
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let line = if new {
        format!("{salt}\n{}\n", hex32(digest))
    } else {
        format!("{}\n", hex32(digest))
    };
    let _ = f.write_all(line.as_bytes());
}

fn read_since(dir: &Path) -> Option<u64> {
    std::fs::read_to_string(dir.join(SINCE_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn write_since(dir: &Path, t: u64) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(SINCE_FILE), t.to_string());
}

/// Every string of `v` cleaned.
fn clean_value(s: &Sanitizer, v: Value) -> Value {
    match v {
        Value::String(t) => Value::String(cut(&s.clean(&t), MAX_MESSAGE)),
        Value::Array(a) => Value::Array(a.into_iter().map(|x| clean_value(s, x)).collect()),
        Value::Object(m) => {
            Value::Object(m.into_iter().map(|(k, x)| (k, clean_value(s, x))).collect())
        }
        other => other,
    }
}

impl Shared {
    /// The event of `e`, cleaned and cut to the limits.
    fn finish(&self, e: NewEvent, tail: Option<String>) -> Event {
        let s = &self.sanitizer;
        let context = e
            .context
            .map(|c| clean_value(s, c))
            .filter(|c| c.to_string().len() <= MAX_CONTEXT);
        Event {
            id: new_id(),
            kind: e.kind,
            at: crate::now_rfc3339(),
            source: e.source,
            code: clean_code(&e.code),
            message: cut(&s.clean(&e.message), MAX_MESSAGE),
            command: e.command.map(|c| cut(&s.clean(&c), 200)),
            module: e.module.map(|m| cut(&s.clean(&m), 64)),
            backtrace: e.backtrace.map(|b| cut(&s.clean(&b), MAX_BACKTRACE)),
            log_tail: tail
                .filter(|t| !t.trim().is_empty())
                .map(|t| cut_front(&s.clean(&t), MAX_LOG_TAIL)),
            context,
        }
    }
}

impl Diagnostics {
    /// The diagnostics with `config`, on when `consent` says so (a missing
    /// installation number is made).
    pub fn new(config: Config, consent: &Consent) -> Self {
        let enabled = consent.enabled == Some(true);
        let install_id = enabled.then(|| {
            consent
                .install_id
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(new_id)
        });
        if enabled && read_since(&config.dir).is_none() {
            write_since(&config.dir, crate::now_unix());
        }
        // the codes of earlier runs, before any crash file or log tail of
        // theirs is queued
        let mut config = config;
        let (salt, digests) = read_redact(&config.dir);
        config.sanitizer.remember(&salt, digests);
        Self {
            inner: Arc::new(Inner {
                enabled: AtomicBool::new(enabled),
                started: Instant::now(),
                queue: Queue::new(config.dir.join("queue")),
                dir: config.dir,
                log_dir: config.log_dir,
                shared: Mutex::new(Shared {
                    install_id,
                    endpoint: config.endpoint,
                    app: config.app,
                    platform: config.platform,
                    sanitizer: config.sanitizer,
                    last: None,
                    recorded: 0,
                    startup_ms: None,
                    session_recorded: false,
                }),
                tail: Mutex::new(VecDeque::new()),
                sending: Mutex::new(()),
                worker: Mutex::new(Worker {
                    running: false,
                    send_now: true,
                    first_new: None,
                    last_new: None,
                    retry_at: None,
                    backoff: FIRST_BACKOFF,
                    next_periodic: Instant::now() + PERIOD,
                }),
                wake: Condvar::new(),
            }),
        }
    }

    fn shared(&self) -> MutexGuard<'_, Shared> {
        take(&self.inner.shared, true).expect("a blocking lock")
    }

    /// `true` while the user agreed: only then is anything collected or
    /// sent.
    pub fn enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Acquire)
    }

    /// The random number of this installation (while on).
    pub fn install_id(&self) -> Option<String> {
        self.shared().install_id.clone()
    }

    /// Where the reports go.
    pub fn endpoint(&self) -> String {
        self.shared().endpoint.clone()
    }

    /// Sends to another address from now on.
    pub fn set_endpoint(&self, url: impl Into<String>) {
        self.shared().endpoint = url.into();
    }

    /// The graphics adapter the app draws with.
    pub fn set_gpu(&self, gpu: Option<Gpu>) {
        self.shared().platform.gpu = gpu;
    }

    /// From now on `word` (a patient code, a case reference) is removed
    /// from every text, in the next runs too (its salted digest is kept in
    /// the diagnostics folder).
    pub fn redact(&self, word: &str) {
        let mut shared = self.shared();
        if shared.sanitizer.redact(word) {
            let salt = shared.sanitizer.salt().to_owned();
            drop(shared);
            append_redact(&self.inner.dir, &salt, &Sanitizer::digest(&salt, word));
        }
    }

    /// The `tracing` layer that records the log's errors.
    pub fn layer(&self) -> DiagLayer {
        DiagLayer { diag: self.clone() }
    }

    /// The events waiting to be sent.
    pub fn pending(&self) -> usize {
        self.inner.queue.len()
    }

    /// The events waiting to be sent, oldest first.
    pub fn queued(&self) -> Vec<Event> {
        self.inner
            .queue
            .batch(usize::MAX, usize::MAX)
            .into_iter()
            .map(|(_, e)| e)
            .collect()
    }

    /// Turns diagnostics on or off and returns the installation number.
    /// On: a new random number (unless already on), crash files from now
    /// on are reported, the start of this run is recorded. Off: the waiting
    /// events, the kept log lines and the number are deleted.
    pub fn set_enabled(&self, on: bool) -> Option<String> {
        if on {
            if !self.enabled() {
                self.shared().install_id = Some(new_id());
                write_since(&self.inner.dir, crate::now_unix());
                self.inner.enabled.store(true, Ordering::Release);
                // the start of this run goes after the debounce, as any event
                self.record_session();
            }
        } else {
            self.inner.enabled.store(false, Ordering::Release);
            {
                let mut sh = self.shared();
                sh.install_id = None;
                sh.last = None;
            }
            self.inner.queue.clear();
            take(&self.inner.tail, true)
                .expect("a blocking lock")
                .clear();
        }
        self.install_id()
    }

    /// The app is on screen: records the session event (`start`, with the
    /// time since [`Diagnostics::new`]), now or once diagnostics are turned
    /// on during this run.
    pub fn launched(&self) {
        let ms = u64::try_from(self.inner.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.shared().startup_ms.get_or_insert(ms);
        self.record_session();
    }

    fn record_session(&self) {
        if !self.enabled() {
            return;
        }
        let ms = {
            let mut sh = self.shared();
            match sh.startup_ms {
                Some(ms) if !sh.session_recorded => {
                    sh.session_recorded = true;
                    ms
                }
                _ => return,
            }
        };
        self.record(
            NewEvent::new(Kind::Session, Source::App, "start", "DentalCraft started")
                .context(json!({ "startup_ms": ms })),
        );
    }

    /// Keeps a log line (the layer calls it while on).
    pub(crate) fn push_log_line(&self, line: &str) {
        if let Some(mut t) = take(&self.inner.tail, false) {
            t.push_back(cut(line, TAIL_LINE_MAX));
            while t.len() > TAIL_LINES {
                t.pop_front();
            }
        }
    }

    fn recent_lines(&self, n: usize, wait: bool) -> Option<String> {
        let t = take(&self.inner.tail, wait)?;
        let skip = t.len().saturating_sub(n);
        Some(t.iter().skip(skip).cloned().collect::<Vec<_>>().join("\n"))
    }

    /// Queues `e` (cleaned, cut to the limits): `false` when off, when it
    /// repeats the previous event within a minute, past the events of one
    /// run, or when the queue cannot be written. Errors and crashes without
    /// log lines get the recent ones.
    pub fn record(&self, e: NewEvent) -> bool {
        self.record_with(e, true)
    }

    fn record_with(&self, mut e: NewEvent, wait: bool) -> bool {
        if !self.enabled() {
            return false;
        }
        if e.log_tail.is_none() && matches!(e.kind, Kind::Error | Kind::Crash) {
            e.log_tail = self.recent_lines(ERROR_TAIL_LINES, wait);
        }
        let event = {
            let Some(mut sh) = take(&self.inner.shared, wait) else {
                return false;
            };
            if matches!(e.kind, Kind::Error | Kind::Warning) {
                let again = sh.last.as_ref().is_some_and(|(k, s, c, m, at)| {
                    (*k, *s) == (e.kind, e.source)
                        && *c == e.code
                        && *m == e.message
                        && at.elapsed() < DEDUP_WINDOW
                });
                if again || sh.recorded >= MAX_PER_RUN {
                    return false;
                }
                sh.last = Some((
                    e.kind,
                    e.source,
                    e.code.clone(),
                    e.message.clone(),
                    Instant::now(),
                ));
                sh.recorded += 1;
            }
            let tail = e.log_tail.take();
            sh.finish(e, tail)
        };
        if self.inner.queue.push(&event).is_err() {
            return false;
        }
        if wait {
            self.notify(false);
        }
        true
    }

    /// The panic hook: queues the crash (`false` when off or when the
    /// diagnostics are busy in this very moment; the crash file is then
    /// picked up at the next start).
    pub fn record_crash(&self, crash: &Crash) -> bool {
        self.record_with(crash.event(false), false)
    }

    /// At start: the crash files the earlier runs left in the log folder
    /// since the consent become crash events (with the last lines of the
    /// newest log) and are renamed `crash-<time>.reported.log`. Returns how
    /// many.
    pub fn pick_up_crashes(&self) -> usize {
        if !self.enabled() {
            return 0;
        }
        let since = read_since(&self.inner.dir).unwrap_or_else(|| {
            let now = crate::now_unix();
            write_since(&self.inner.dir, now);
            now
        });
        let files = crash::unreported(&self.inner.log_dir, since);
        if files.is_empty() {
            return 0;
        }
        let tail = crate::logs::last_lines(&self.inner.log_dir, CRASH_TAIL_LINES).join("\n");
        let mut n = 0;
        for path in files {
            let Some(c) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| Crash::parse(&t))
            else {
                continue;
            };
            let mut e = c.event(true);
            e.log_tail = Some(tail.clone());
            if self.record(e) {
                let _ = std::fs::rename(&path, crash::reported_name(&path));
                n += 1;
            }
        }
        n
    }

    fn body(&self, events: &[Event]) -> Option<Vec<u8>> {
        let sh = self.shared();
        let install_id = sh.install_id.as_deref()?;
        let mut system = sh.platform.clone();
        system.locale = dc_i18n::current().code().to_owned();
        serde_json::to_vec(&ReportBody {
            schema: crate::SCHEMA,
            install_id,
            sent_at: crate::now_rfc3339(),
            app: &sh.app,
            system: &system,
            events,
        })
        .ok()
    }

    /// The next report as it would be sent now (`null` when nothing waits
    /// or diagnostics are off), with `enabled` and `pending`.
    pub fn preview(&self) -> Value {
        let batch: Vec<Event> = self
            .inner
            .queue
            .batch(MAX_EVENTS, BATCH_BYTES)
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        let report = if self.enabled() && !batch.is_empty() {
            self.body(&batch)
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        json!({
            "enabled": self.enabled(),
            "pending": self.pending(),
            "report": report,
        })
    }

    /// Sends the waiting events now, in reports of at most [`MAX_EVENTS`]
    /// events and [`MAX_BODY`] bytes, until none waits or the server
    /// cannot take more. `Err` when diagnostics are off.
    pub fn flush(&self) -> Result<Flush, String> {
        let _one_at_a_time = take(&self.inner.sending, true);
        if !self.enabled() {
            return Err("diagnostics are off: nothing is collected or sent".into());
        }
        let endpoint = self.endpoint();
        if !crate::endpoint_allowed(&endpoint) {
            return Err(format!(
                "the diagnostics address {endpoint} is not https (http only on this computer): nothing is sent"
            ));
        }
        let mut out = Flush::default();
        for _ in 0..MAX_BATCHES {
            let batch = self.inner.queue.batch(MAX_EVENTS, BATCH_BYTES);
            if batch.is_empty() {
                break;
            }
            let (paths, events): (Vec<PathBuf>, Vec<Event>) = batch.into_iter().unzip();
            let Some(body) = self.body(&events) else {
                break;
            };
            match crate::post(&endpoint, &body) {
                SendOutcome::Accepted {
                    accepted,
                    duplicates,
                } => {
                    self.inner.queue.remove(&paths);
                    out.sent += events.len();
                    out.accepted += accepted;
                    out.duplicates += duplicates;
                }
                SendOutcome::Rejected(status) => {
                    tracing::warn!(
                        "diagnostics: the server refused {} events (HTTP {status}); they are dropped",
                        events.len()
                    );
                    self.inner.queue.remove(&paths);
                    out.dropped += events.len();
                }
                SendOutcome::RetryLater { after, reason } => {
                    tracing::info!("diagnostics not sent now ({reason}), later");
                    out.retry = Some(reason);
                    out.retry_after = after;
                    break;
                }
            }
            if !self.enabled() {
                break;
            }
        }
        out.remaining = self.pending();
        Ok(out)
    }

    /// Wakes the worker: `now` sends at once, else after the debounce.
    fn notify(&self, now: bool) {
        let mut w = take(&self.inner.worker, true).expect("a blocking lock");
        if now {
            w.send_now = true;
        } else {
            let t = Instant::now();
            w.first_new.get_or_insert(t);
            w.last_new = Some(t);
        }
        drop(w);
        self.inner.wake.notify_all();
    }

    /// Asks the worker to send now (it waits for a refusal's delay).
    pub fn send_soon(&self) {
        self.notify(true);
    }

    /// Starts the thread that sends the reports (once): at start, a few
    /// seconds after new events, every ten minutes, and later again after a
    /// failure (60 s doubling to an hour, or the server's `Retry-After`).
    /// It sleeps while diagnostics are off and ends with the last handle.
    pub fn start_worker(&self) {
        {
            let mut w = take(&self.inner.worker, true).expect("a blocking lock");
            if w.running {
                return;
            }
            w.running = true;
        }
        let weak = Arc::downgrade(&self.inner);
        let spawned = std::thread::Builder::new()
            .name("dc-diagnostics".into())
            .spawn(move || worker_loop(&weak));
        if spawned.is_err() {
            take(&self.inner.worker, true)
                .expect("a blocking lock")
                .running = false;
        }
    }
}

fn worker_loop(weak: &std::sync::Weak<Inner>) {
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let diag = Diagnostics { inner };
        {
            let mut w = take(&diag.inner.worker, true).expect("a blocking lock");
            loop {
                // the app is gone: only this thread holds the state
                if Arc::strong_count(&diag.inner) <= 1 {
                    return;
                }
                let now = Instant::now();
                let due = w.due(now);
                if diag.enabled() && due <= now {
                    w.send_now = false;
                    w.first_new = None;
                    w.last_new = None;
                    w.next_periodic = now + PERIOD;
                    break;
                }
                let wait = if diag.enabled() {
                    due.saturating_duration_since(now)
                } else {
                    // off at start: nothing to send at start
                    w.send_now = false;
                    PERIOD
                };
                // wake up now and then to notice the end of the app
                let wait = wait.min(Duration::from_secs(30));
                w = diag
                    .inner
                    .wake
                    .wait_timeout(w, wait)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
        }
        let result = if diag.inner.queue.is_empty() {
            Ok(Flush::default())
        } else {
            diag.flush()
        };
        let mut w = take(&diag.inner.worker, true).expect("a blocking lock");
        match result {
            Ok(f) if f.retry.is_some() => {
                let wait = f.retry_after.map_or(w.backoff, |a| a.min(MAX_RETRY_AFTER));
                w.retry_at = Some(Instant::now() + wait);
                w.backoff = (w.backoff * 2).min(MAX_BACKOFF);
                // what waits goes after the delay
                w.first_new.get_or_insert_with(Instant::now);
                w.last_new.get_or_insert_with(Instant::now);
            }
            _ => {
                w.retry_at = None;
                w.backoff = FIRST_BACKOFF;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::send::test_server::serve;

    /// Diagnostics under `root`, reports to a closed port.
    pub(crate) fn diagnostics(root: &Path, on: bool) -> Diagnostics {
        let config = Config {
            dir: root.join("diagnostics"),
            log_dir: root.join("logs"),
            endpoint: crate::endpoint("http://127.0.0.1:9"),
            app: AppInfo {
                version: "0.0.0-test".into(),
                commit: None,
                install_kind: Some("dev".into()),
            },
            platform: Platform::current(None),
            sanitizer: Sanitizer::new(Some("/home/mrossi"), Some("mrossi")),
        };
        Diagnostics::new(
            config,
            &Consent {
                enabled: Some(on),
                install_id: None,
            },
        )
    }

    fn error(message: &str) -> NewEvent {
        NewEvent::new(Kind::Error, Source::Save, "save-failed", message)
    }

    #[test]
    fn reports_never_go_to_plain_http_off_this_computer() {
        for ok in [
            "https://dentalcraft.app/api/v1/diagnostics/reports",
            "http://127.0.0.1:8787/api/v1/diagnostics/reports",
            "http://localhost:8787/x",
            "http://[::1]:8787/x",
        ] {
            assert!(crate::endpoint_allowed(ok), "{ok}");
        }
        for bad in [
            "http://x/api/v1/diagnostics/reports",
            "http://127.0.0.1.evil.example/x",
            "ftp://dentalcraft.app/x",
            "not a url",
            "",
        ] {
            assert!(!crate::endpoint_allowed(bad), "{bad}");
        }
        let dir = tempfile::tempdir().unwrap();
        let d = diagnostics(dir.path(), true);
        d.set_endpoint(crate::endpoint("http://x"));
        assert!(d.record(error("a")));
        let before = d.pending();
        assert!(d.flush().unwrap_err().contains("not https"));
        assert_eq!(d.pending(), before, "nothing sent, nothing dropped");
    }

    #[test]
    fn codes_of_earlier_runs_are_redacted_before_anything_is_queued() {
        let dir = tempfile::tempdir().unwrap();
        let file =
            || std::fs::read_to_string(dir.path().join("diagnostics").join(REDACT_FILE)).unwrap();
        {
            let d = diagnostics(dir.path(), false);
            d.redact("AL-2026-0142");
            d.redact("BIA-07");
        }
        // the file never holds the codes themselves
        let first = file();
        assert!(
            !first.contains("AL-2026") && !first.contains("BIA"),
            "{first}"
        );
        assert_eq!(first.lines().count(), 3, "salt and two digests: {first}");
        // the next run, before any project is open
        let d = diagnostics(dir.path(), true);
        assert!(d.record(error(
            "panicked: case AL-2026-0142_BIA-07 at step 3, see AL-2026-0142."
        )));
        assert_eq!(
            d.queued().last().unwrap().message,
            "panicked: case <case>_<case> at step 3, see <case>."
        );
        // a code known from an earlier run is not written again
        d.redact("BIA-07");
        assert_eq!(file(), first);
    }

    #[test]
    fn nothing_is_kept_without_consent_and_turning_off_forgets() {
        let dir = tempfile::tempdir().unwrap();
        let d = diagnostics(dir.path(), false);
        assert!(!d.record(error("x")));
        d.launched();
        assert_eq!((d.pending(), d.install_id()), (0, None));
        assert_eq!(d.preview()["report"], Value::Null);
        assert!(d.flush().is_err());
        // on: a new number, and the start of this run
        let id = d.set_enabled(true).unwrap();
        assert_eq!(uuid::Uuid::parse_str(&id).unwrap().get_version_num(), 4);
        let q = d.queued();
        assert_eq!(q.len(), 1);
        assert_eq!((q[0].kind, q[0].code.as_str()), (Kind::Session, "start"));
        assert!(q[0].context.as_ref().unwrap()["startup_ms"].is_u64());
        // on again: same number, no second session
        assert_eq!(d.set_enabled(true).as_deref(), Some(id.as_str()));
        assert!(d.record(error("cannot save /home/mrossi/Cases/AL-1.dcprj: denied")));
        assert_eq!(d.queued()[1].message, "cannot save <project>: denied");
        // off: the queue and the number go
        assert_eq!(d.set_enabled(false), None);
        assert_eq!(d.pending(), 0);
        // on again: another number
        assert_ne!(d.set_enabled(true), Some(id));
    }

    #[test]
    fn repeats_are_dropped_and_texts_cut() {
        let dir = tempfile::tempdir().unwrap();
        let d = diagnostics(dir.path(), true);
        assert!(d.record(error("a")));
        assert!(!d.record(error("a")));
        assert!(d.record(error("b")));
        assert!(d.record(error("a")));
        let mut big = error(&"x".repeat(5000));
        big.code = "Some Code With Spaces".into();
        big.backtrace = Some("y".repeat(100_000));
        big.context =
            Some(json!({ "a": "z".repeat(1900), "b": "z".repeat(1900), "c": "z".repeat(1900) }));
        assert!(d.record(big));
        let e = d.queued().pop().unwrap();
        assert!(e.message.len() <= MAX_MESSAGE);
        assert!(e.backtrace.unwrap().len() <= MAX_BACKTRACE);
        assert_eq!(e.code, "some-code-with-spaces");
        // a context over 4 KiB is left out
        assert_eq!(e.context, None);
        // the case words registered by the app
        d.redact("BIA-07");
        assert!(d.record(error("case BIA-07 broke")));
        assert_eq!(d.queued().pop().unwrap().message, "case <case> broke");
    }

    #[test]
    fn the_server_takes_refuses_or_delays_the_reports() {
        let dir = tempfile::tempdir().unwrap();
        let d = diagnostics(dir.path(), true);
        for m in ["a", "b", "c"] {
            d.record(error(m));
        }
        let (base, got) = serve(vec![
            (429, "Retry-After: 30\r\n", String::new()),
            (202, "", r#"{"accepted":2,"duplicates":1}"#.into()),
            (400, "", r#"{"error":"invalid","detail":"x"}"#.into()),
        ]);
        d.set_endpoint(crate::endpoint(&base));
        // busy: everything stays
        let f = d.flush().unwrap();
        assert_eq!((f.sent, f.remaining), (0, 3));
        assert_eq!(f.retry_after, Some(Duration::from_secs(30)));
        // taken: the queue is empty
        let f = d.flush().unwrap();
        assert_eq!(
            (f.sent, f.accepted, f.duplicates, f.remaining),
            (3, 2, 1, 0)
        );
        assert_eq!(f.retry, None);
        // refused: dropped
        d.record(error("d"));
        let f = d.flush().unwrap();
        assert_eq!((f.sent, f.dropped, f.remaining), (0, 1, 0));
        // what the server got: the contract's report, anonymous
        let got = got.lock().unwrap();
        let body: Value = serde_json::from_str(&got[1].body).unwrap();
        assert_eq!(body["schema"], 1);
        assert_eq!(body["install_id"], d.install_id().unwrap());
        assert_eq!(body["app"]["version"], "0.0.0-test");
        assert!(body["system"]["arch"].is_string() && body["system"]["locale"].is_string());
        assert!(body["sent_at"].as_str().unwrap().ends_with('Z'));
        assert_eq!(body["events"].as_array().unwrap().len(), 3);
        assert!(!got[1].head.to_ascii_lowercase().contains("authorization"));
        // the preview is what would be sent
        d.record(error("e"));
        let p = d.preview();
        assert_eq!(p["pending"], 1);
        assert_eq!(p["report"]["events"][0]["message"], "e");
    }

    #[test]
    fn the_worker_sends_by_itself() {
        let dir = tempfile::tempdir().unwrap();
        let d = diagnostics(dir.path(), true);
        let (base, got) = serve(vec![(202, "", r#"{"accepted":1,"duplicates":0}"#.into())]);
        d.set_endpoint(crate::endpoint(&base));
        d.record(error("a"));
        d.start_worker();
        let deadline = Instant::now() + Duration::from_secs(20);
        while d.pending() > 0 {
            assert!(Instant::now() < deadline, "the worker did not send");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(got.lock().unwrap().len(), 1);
    }

    /// End to end against a running `dentalcraft-server dev`:
    /// `DC_DIAGNOSTICS_IT=http://127.0.0.1:8787 cargo test -p dc-diagnostics
    /// -- --ignored a_running_server`.
    #[test]
    #[ignore = "needs a running dentalcraft-server (DC_DIAGNOSTICS_IT)"]
    fn a_running_server_takes_the_reports() {
        let Ok(issuer) = std::env::var("DC_DIAGNOSTICS_IT") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let d = diagnostics(dir.path(), false);
        d.set_endpoint(crate::endpoint(&issuer));
        d.launched();
        d.set_enabled(true);
        d.record(
            error("cannot save /home/mrossi/Cases/x.dcprj: denied")
                .command("aligners.planning.setup")
                .module("aligners"),
        );
        let crash = "DentalCraft 0.1.0 crashed\nthread: main\npanicked at crates/dc-app/src/app.rs:1:1:\nboom\n\nbacktrace:\n   0: dc_app::app\n";
        assert!(d.record_crash(&Crash::parse(crash).unwrap()));
        let f = d.flush().unwrap();
        assert_eq!(
            (f.retry.clone(), f.dropped, f.remaining),
            (None, 0, 0),
            "{f:?}"
        );
        assert_eq!((f.sent, f.accepted), (3, 3), "{f:?}");
    }

    #[test]
    fn crashes_of_earlier_runs_are_picked_up_once() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let crash = "DentalCraft 0.1.0 crashed\nthread: main\npanicked at crates/dc-app/src/app.rs:1:1:\nboom in /home/mrossi/Cases/x.dcprj\n\nbacktrace:\n   0: dc_app::app\n";
        // before the consent: never reported
        std::fs::write(logs.join("crash-10.log"), crash).unwrap();
        std::fs::write(
            logs.join("dentalcraft.log.2026-11-18"),
            "INFO signed in as mario@studio.it\nERROR about to crash\n",
        )
        .unwrap();
        let d = diagnostics(dir.path(), false);
        assert_eq!(d.pick_up_crashes(), 0);
        d.set_enabled(true);
        let now = crate::now_unix();
        std::fs::write(logs.join(format!("crash-{now}.log")), crash).unwrap();
        assert_eq!(d.pick_up_crashes(), 1);
        assert_eq!(d.pick_up_crashes(), 0);
        assert!(logs.join("crash-10.log").exists());
        assert!(logs.join(format!("crash-{now}.reported.log")).exists());
        let e = d
            .queued()
            .into_iter()
            .find(|e| e.kind == Kind::Crash)
            .unwrap();
        assert_eq!(e.code, "panic");
        assert_eq!(e.source, Source::App);
        assert!(e.message.ends_with("boom in <project>"), "{}", e.message);
        assert!(e.backtrace.unwrap().contains("dc_app::app"));
        let tail = e.log_tail.unwrap();
        assert!(
            tail.contains("about to crash") && tail.contains("<email>"),
            "{tail}"
        );
        // the panic hook of this run
        let c = Crash::parse(crash).unwrap();
        assert!(d.record_crash(&c));
        d.set_enabled(false);
        assert!(!d.record_crash(&c));
    }
}
