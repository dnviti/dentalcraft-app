//! Removes personal data from a text before it is queued.
//!
//! In order: the patient codes and case references the app registered
//! ([`Sanitizer::redact`]) become `<case>`; the home folder becomes `~`;
//! e-mail addresses become `<email>`; access codes (`dcat_…`, `Bearer …`,
//! `access_token=…`, long random strings) become `<token>`; every absolute
//! path (Unix, Windows drive, UNC, network mounts, other users' folders),
//! quoted texts naming a project or scan file, and project and scan file
//! names become `<project>`; serial numbers and MAC addresses become
//! `<serial>`; the user name becomes `<user>`. Only an allow-list of folders
//! keeps its path: DentalCraft's own (logs, settings, install), the
//! system's and the Rust toolchain's; they help to find a bug and name
//! nobody.

/// Cleans texts for the reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sanitizer {
    /// The home folder, as written by the platform and with `/`.
    home: Vec<String>,
    /// The user's login name.
    user: Option<String>,
    /// Registered words (patient codes, case references), longest first.
    words: Vec<String>,
    /// Salt of [`Sanitizer::digest`] (one per installation).
    salt: String,
    /// Salted digests of the words registered in earlier runs (the plain
    /// words are never written to disk): a whole word with one of these
    /// digests becomes `<case>` too.
    digests: std::collections::BTreeSet<[u8; 32]>,
}

/// Extensions of project and scan files.
const DATA_EXTENSIONS: [&str; 11] = [
    ".dcprj", ".stl", ".ply", ".obj", ".off", ".3mf", ".dcm", ".glb", ".gltf", ".xyz", ".pcd",
];

/// Folders under the home folder that name nobody: DentalCraft's own and the
/// Rust toolchain's (lower case, `/` separators, after `~/`).
const KEPT_HOME_DIRS: [&str; 10] = [
    ".cargo/",
    ".rustup/",
    ".local/share/dentalcraft",
    ".config/dentalcraft",
    ".cache/dentalcraft",
    ".var/app/com.dentalcraft.dentalcraft",
    "appdata/roaming/dentalcraft",
    "appdata/local/dentalcraft",
    "library/application support/app.dentalcraft.dentalcraft",
    "library/caches/app.dentalcraft.dentalcraft",
];

/// Absolute Unix folders that name nobody (system, install and toolchain
/// folders): a path under them stays. Every other absolute path goes.
const KEPT_ROOTS: [&str; 13] = [
    "/usr/",
    "/lib/",
    "/lib64/",
    "/bin/",
    "/sbin/",
    "/etc/",
    "/proc/",
    "/sys/",
    "/dev/",
    "/rustc/",
    "/app/",
    "/opt/dentalcraft/",
    "/Applications/DentalCraft.app/",
];

/// Toolchain folders that may sit in anybody's home (a backtrace of a
/// dependency built elsewhere): the path keeps its tail from here, the
/// folders before it go.
const TOOLCHAIN_MARKERS: [&str; 3] = ["/.cargo/registry/", "/.cargo/git/", "/.rustup/toolchains/"];

/// Keys whose value is a secret wherever they appear.
const SECRET_KEYS: [&str; 6] = [
    "access_token",
    "refresh_token",
    "id_token",
    "client_secret",
    "code_verifier",
    "password",
];

/// Query keys whose value is a secret in a URL (`?code=…&state=…`).
const QUERY_SECRET_KEYS: [&str; 3] = ["code", "state", "token"];

/// Prefixes of DentalCraft's access codes.
const TOKEN_PREFIXES: [&str; 4] = ["dcat_", "dcrt_", "dcmcp_", "dcac_"];

impl Sanitizer {
    /// For the running user: the home folder of the platform and the login
    /// name (`$USER`, `%USERNAME%`, else the home folder's name).
    pub fn current() -> Self {
        let home =
            directories::UserDirs::new().map(|u| u.home_dir().to_string_lossy().into_owned());
        let user = std::env::var("USER")
            .ok()
            .or_else(|| std::env::var("USERNAME").ok())
            .or_else(|| {
                home.as_deref()
                    .and_then(|h| h.rsplit(['/', '\\']).find(|s| !s.is_empty()))
                    .map(str::to_owned)
            });
        Self::new(home.as_deref(), user.as_deref())
    }

    /// For the home folder `home` and the login name `user`.
    pub fn new(home: Option<&str>, user: Option<&str>) -> Self {
        let mut homes = Vec::new();
        if let Some(h) = home
            .map(|h| h.trim_end_matches(['/', '\\']))
            .filter(|h| h.len() > 1)
        {
            homes.push(h.to_owned());
            let slashes = h.replace('\\', "/");
            if slashes != h {
                homes.push(slashes);
            }
        }
        Self {
            home: homes,
            user: user
                .map(str::trim)
                .filter(|u| u.chars().count() >= 2)
                .map(str::to_owned),
            words: Vec::new(),
            salt: String::new(),
            digests: Default::default(),
        }
    }

    /// From now on `word` (a patient code, a case reference) becomes
    /// `<case>`. Words shorter than three characters are ignored (they
    /// would hide too much); the file name rules still cover them in file
    /// names. `true` when the word is new and not remembered from an earlier
    /// run (see [`Sanitizer::remember`]).
    pub fn redact(&mut self, word: &str) -> bool {
        let w = word.trim();
        if w.chars().count() < 3 || self.words.iter().any(|x| x == w) {
            return false;
        }
        self.words.push(w.to_owned());
        self.words.sort_by_key(|w| std::cmp::Reverse(w.len()));
        !self.digests.contains(&Self::digest(&self.salt, w))
    }

    /// The salted digest of `word` (trimmed), as written to disk.
    pub fn digest(salt: &str, word: &str) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"dentalcraft-redact\0");
        h.update(salt.as_bytes());
        h.update(b"\0");
        h.update(word.trim().as_bytes());
        h.finalize().into()
    }

    /// The salt of the digests and the digests of the words registered in
    /// earlier runs.
    pub fn remember(&mut self, salt: &str, digests: impl IntoIterator<Item = [u8; 32]>) {
        salt.clone_into(&mut self.salt);
        self.digests.extend(digests);
    }

    /// The salt set by [`Sanitizer::remember`].
    pub fn salt(&self) -> &str {
        &self.salt
    }

    /// `text` without personal data.
    pub fn clean(&self, text: &str) -> String {
        let mut s = text.to_owned();
        for w in &self.words {
            s = s.replace(w.as_str(), "<case>");
        }
        if !self.digests.is_empty() {
            s = self.remembered_words(&s);
        }
        for h in &self.home {
            s = replace_where(&s, h, "~", |_, after| {
                after.is_none_or(|c| !(c.is_alphanumeric() || "_-.".contains(c)))
            });
        }
        s = emails(&s);
        s = tokens(&s);
        s = serials(&s);
        s = user_paths(&s);
        s = quoted(&s);
        s = data_files(&s);
        if let Some(u) = &self.user {
            let word = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            s = replace_where(&s, u, "<user>", |before, after| word(before) && word(after));
        }
        s
    }
}

impl Sanitizer {
    /// Words of `s` (runs of letters, digits, `-`, `_`, `.`, and their parts
    /// between `_` and `.`) whose digest was remembered become `<case>`.
    fn remembered_words(&self, s: &str) -> String {
        let known =
            |w: &str| w.chars().count() >= 3 && self.digests.contains(&Self::digest(&self.salt, w));
        let run = |c: char| c.is_alphanumeric() || "-_.".contains(c);
        let mut spans = Vec::new();
        let mut start = None;
        let chars: Vec<(usize, char)> = s.char_indices().collect();
        for (k, &(i, c)) in chars.iter().enumerate() {
            if run(c) {
                start.get_or_insert(i);
            }
            let ends = !run(c) || k + 1 == chars.len();
            if ends && let Some(a) = start.take() {
                let b = if run(c) { i + c.len_utf8() } else { i };
                let whole = s[a..b].trim_end_matches('.');
                if known(whole) {
                    spans.push((a, a + whole.len()));
                    continue;
                }
                let mut from = a;
                for part in s[a..b].split(['_', '.']) {
                    if known(part) {
                        spans.push((from, from + part.len()));
                    }
                    from += part.len() + 1;
                }
            }
        }
        splice(s, &spans, "<case>")
    }
}

/// Replaces each `needle` of `s` for which `ok(char before, char after)`.
fn replace_where(
    s: &str,
    needle: &str,
    rep: &str,
    ok: impl Fn(Option<char>, Option<char>) -> bool,
) -> String {
    if needle.is_empty() {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let (mut kept, mut from) = (0, 0);
    while let Some(i) = s[from..].find(needle) {
        let at = from + i;
        let end = at + needle.len();
        if ok(s[..at].chars().next_back(), s[end..].chars().next()) {
            out.push_str(&s[kept..at]);
            out.push_str(rep);
            kept = end;
            from = end;
        } else {
            from = at + s[at..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out.push_str(&s[kept..]);
    out
}

/// Replaces the byte ranges `spans` (sorted, not overlapping) of `s`.
fn splice(s: &str, spans: &[(usize, usize)], rep: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut kept = 0;
    for &(a, b) in spans {
        out.push_str(&s[kept..a]);
        out.push_str(rep);
        kept = b;
    }
    out.push_str(&s[kept..]);
    out
}

fn emails(s: &str) -> String {
    let b = s.as_bytes();
    let local = |c: u8| c.is_ascii_alphanumeric() || b"._%+-".contains(&c);
    let domain = |c: u8| c.is_ascii_alphanumeric() || c == b'.' || c == b'-';
    let mut spans = Vec::new();
    let mut floor = 0;
    for (i, _) in s.match_indices('@') {
        if i < floor {
            continue;
        }
        let mut start = i;
        while start > floor && local(b[start - 1]) {
            start -= 1;
        }
        let mut end = i + 1;
        while end < b.len() && domain(b[end]) {
            end += 1;
        }
        while end > i + 1 && matches!(b[end - 1], b'.' | b'-') {
            end -= 1;
        }
        let dom = &s[i + 1..end];
        let tld_ok = dom.rsplit_once('.').is_some_and(|(host, tld)| {
            !host.is_empty() && tld.len() >= 2 && tld.bytes().all(|c| c.is_ascii_alphabetic())
        });
        if start < i && tld_ok {
            spans.push((start, end));
            floor = end;
        }
    }
    splice(s, &spans, "<email>")
}

fn tokens(s: &str) -> String {
    let mut s = s.to_owned();
    // `key=value`, `"key":"value"`, `key: value`
    for key in SECRET_KEYS {
        s = secret_values(&s, key, false, "<token>");
    }
    for key in QUERY_SECRET_KEYS {
        s = secret_values(&s, key, true, "<token>");
    }
    // runs that look like a token
    let run = |c: char| c.is_ascii_alphanumeric() || "_-+/=.".contains(c);
    let mut spans = Vec::new();
    let mut start = None;
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    for (k, &(i, c)) in chars.iter().enumerate() {
        if run(c) {
            start.get_or_insert(i);
        }
        let ends = !run(c) || k + 1 == chars.len();
        if ends && let Some(a) = start.take() {
            let b = if run(c) { i + c.len_utf8() } else { i };
            let word = &s[a..b];
            let bearer = word.len() >= 8
                && a >= 7
                && s.get(a - 7..a)
                    .is_some_and(|p| p.eq_ignore_ascii_case("bearer "));
            if bearer || looks_like_token(word) {
                spans.push((a, b));
            }
        }
    }
    splice(&s, &spans, "<token>")
}

/// A DentalCraft access code, a JWT, or a long random mix of letters and
/// digits (hex digests and UUIDs, all lower case, stay).
fn looks_like_token(w: &str) -> bool {
    let w = w.trim_end_matches('.');
    if TOKEN_PREFIXES.iter().any(|p| w.starts_with(p)) && w.len() > 8 {
        return true;
    }
    if w.starts_with("eyJ") && w.len() >= 20 {
        return true;
    }
    w.len() >= 40
        && w.bytes().any(|c| c.is_ascii_uppercase())
        && w.bytes().any(|c| c.is_ascii_lowercase())
        && w.bytes().any(|c| c.is_ascii_digit())
        && w.matches('/').count() <= 1
}

/// Replaces the values of `key` (`key=v`, `"key":"v"`, `key: v`) with
/// `rep`; with `query`, only after `?` or `&`.
fn secret_values(s: &str, key: &str, query: bool, rep: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let mut spans = Vec::new();
    for (at, _) in lower.match_indices(key) {
        let before = lower[..at].chars().next_back();
        if query {
            if !matches!(before, Some('?' | '&')) {
                continue;
            }
        } else if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let mut i = at + key.len();
        let rest = &s[i..];
        let sep = if query {
            rest.starts_with('=').then_some(1)
        } else if let Some(r) = rest.strip_prefix("\":") {
            let value = r.trim_start();
            value.starts_with('"').then(|| rest.len() - value.len() + 1)
        } else if rest.starts_with('=') {
            Some(1)
        } else if rest.starts_with(": ") {
            Some(2)
        } else {
            None
        };
        let Some(sep) = sep else { continue };
        i += sep;
        let end = s[i..]
            .find(|c: char| c.is_whitespace() || "&\"',;)}".contains(c))
            .map_or(s.len(), |e| i + e);
        if end > i && !s[i..end].starts_with('<') {
            spans.push((i, end));
        }
    }
    spans.sort_unstable();
    spans.dedup();
    splice(s, &spans, rep)
}

/// Keys whose value identifies a device (a scanner's serial number).
const SERIAL_KEYS: [&str; 5] = ["serial_number", "serial", "device_id", "s/n", "sn"];

/// Serial numbers and MAC addresses become `<serial>`: the values of
/// [`SERIAL_KEYS`], `aa:bb:cc:dd:ee:ff` / `AA-BB-…` addresses, and upper
/// case words of ten or more letters and digits mixed (as device serials
/// are written).
fn serials(s: &str) -> String {
    let mut s = s.to_owned();
    for key in SERIAL_KEYS {
        s = secret_values(&s, key, false, "<serial>");
    }
    let word = |c: char| c.is_ascii_alphanumeric() || c == ':' || c == '-';
    let mut spans = Vec::new();
    let mut start = None;
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    for (k, &(i, c)) in chars.iter().enumerate() {
        if word(c) {
            start.get_or_insert(i);
        }
        let ends = !word(c) || k + 1 == chars.len();
        if ends && let Some(a) = start.take() {
            let b = if word(c) { i + c.len_utf8() } else { i };
            for (x, y) in serial_parts(&s[a..b]) {
                spans.push((a + x, a + y));
            }
        }
    }
    splice(&s, &spans, "<serial>")
}

/// Byte ranges of `w` (letters, digits, `:` and `-`) that are a MAC address
/// or a serial number.
fn serial_parts(w: &str) -> Vec<(usize, usize)> {
    let is_mac = |m: &str| {
        let b = m.as_bytes();
        b.len() == 17
            && (0..6).all(|g| b[g * 3].is_ascii_hexdigit() && b[g * 3 + 1].is_ascii_hexdigit())
            && (0..5).all(|g| b[g * 3 + 2] == b[2] && matches!(b[2], b':' | b'-'))
    };
    if is_mac(w) {
        return vec![(0, w.len())];
    }
    let mut out = Vec::new();
    let mut from = 0;
    for part in w.split([':', '-']) {
        let serial = part.len() >= 10
            && part
                .bytes()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            && part.bytes().any(|c| c.is_ascii_uppercase())
            && part.bytes().any(|c| c.is_ascii_digit());
        if serial {
            out.push((from, from + part.len()));
        }
        from += part.len() + 1;
    }
    out
}

/// Where a path that starts at `i` ends: at a line end, a quote, a bracket,
/// a list separator (`, `, `;`, `|`) or a `: ` (paths may hold spaces, and
/// commas, as GNOME's network mounts `smb-share:server=x,share=y` do).
fn path_end(s: &str, i: usize) -> usize {
    let mut end = s.len();
    let mut it = s[i..].char_indices().peekable();
    while let Some((k, c)) = it.next() {
        let next = it.peek().map(|&(_, n)| n);
        let stop = matches!(
            c,
            '\n' | '\r' | '\t' | '"' | '\'' | '`' | ')' | ']' | '>' | '<' | ';' | '|'
        ) || (matches!(c, ':' | ',') && next.is_none_or(char::is_whitespace));
        if stop {
            end = i + k;
            break;
        }
    }
    // a sentence's final full stop and blanks are not part of the path
    let trimmed = s[i..end].trim_end_matches([' ', '.']);
    i + trimmed.len().max(1)
}

/// What happens to an absolute path.
enum PathFate {
    /// It names nobody: it stays.
    Kept,
    /// It goes as a whole.
    Gone,
    /// The folders before byte `n` (a toolchain folder) go.
    Head(usize),
}

/// Absolute paths not on the allow-list: a toolchain folder keeps its tail.
fn gone_or_toolchain(path: &str) -> PathFate {
    let norm = path.replace('\\', "/").to_ascii_lowercase();
    TOOLCHAIN_MARKERS
        .iter()
        .filter_map(|m| norm.find(m))
        .min()
        .map_or(PathFate::Gone, PathFate::Head)
}

fn path_char(c: char) -> bool {
    c.is_alphanumeric() || "._-$?".contains(c)
}

fn user_paths(s: &str) -> String {
    let boundary =
        |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || "_-./\\:~".contains(c)));
    let mut spans: Vec<(usize, usize, &str)> = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let before = s[..i].chars().next_back();
        let after = |n: usize| rest[n..].chars().next().is_some_and(path_char);
        let start = if !boundary(before) {
            None
        } else if rest.starts_with("~/") || rest.starts_with("~\\") {
            let end = path_end(s, i);
            let inner = s[i + 2..end].replace('\\', "/").to_lowercase();
            let kept = KEPT_HOME_DIRS.iter().any(|d| inner.starts_with(d));
            Some((end, if kept { PathFate::Kept } else { PathFate::Gone }))
        } else if (rest.starts_with("\\\\") || rest.starts_with("//")) && after(2) {
            // UNC (`\\server\share\…`): a network share, never allowed
            let end = path_end(s, i);
            Some((end, gone_or_toolchain(&s[i..end])))
        } else if rest.starts_with('/') && after(1) {
            let end = path_end(s, i);
            let path = &s[i..end];
            if !path[1..].contains('/') {
                // `/name`: a single word (a web route, a device), no folder
                None
            } else if KEPT_ROOTS.iter().any(|r| path.starts_with(r)) {
                Some((end, PathFate::Kept))
            } else {
                Some((end, gone_or_toolchain(path)))
            }
        } else if let [d, b':', sep, ..] = rest.as_bytes()
            && d.is_ascii_alphabetic()
            && matches!(sep, b'\\' | b'/')
        {
            let end = path_end(s, i);
            let inner = s[i + 3..end].replace('\\', "/").to_lowercase();
            if inner.starts_with("windows/") || inner.starts_with("program files") {
                Some((end, PathFate::Kept))
            } else {
                Some((end, gone_or_toolchain(&s[i..end])))
            }
        } else {
            None
        };
        match start {
            Some((end, fate)) => {
                match fate {
                    PathFate::Kept => {}
                    PathFate::Gone => spans.push((i, end, "<project>")),
                    PathFate::Head(n) => spans.push((i, i + n, "<path>")),
                }
                i = end.max(i + 1);
            }
            None => i += rest.chars().next().map_or(1, char::len_utf8),
        }
        while !s.is_char_boundary(i) {
            i += 1;
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut kept = 0;
    for (a, b, rep) in spans {
        out.push_str(&s[kept..a]);
        out.push_str(rep);
        kept = b;
    }
    out.push_str(&s[kept..]);
    out
}

/// `true` when `w` names a project or scan file.
fn is_data_file(w: &str) -> bool {
    let lower = w.to_lowercase();
    lower.contains(".dcprj")
        || DATA_EXTENSIONS
            .iter()
            .any(|e| lower.ends_with(e) && lower.len() > e.len())
}

fn file_word(c: char) -> bool {
    c.is_alphanumeric() || "_-.+".contains(c)
}

fn names_data_file(text: &str) -> bool {
    text.split(|c: char| !file_word(c)).any(is_data_file)
}

/// Quoted texts (`"…"`, `` `…` ``, `'…'`) that name a project or scan file:
/// the whole quote goes, file names may hold spaces.
fn quoted(s: &str) -> String {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for q in ['"', '`', '\''] {
        let mut from = 0;
        while let Some(o) = s[from..].find(q) {
            let open = from + o;
            if q == '\''
                && s[..open]
                    .chars()
                    .next_back()
                    .is_some_and(|c| !(c.is_whitespace() || "([{=:".contains(c)))
            {
                from = open + 1;
                continue;
            }
            let Some(c) = s[open + 1..].find(q) else {
                break;
            };
            let close = open + 1 + c;
            let inner = &s[open + 1..close];
            if !inner.contains('\n')
                && names_data_file(inner)
                && !spans.iter().any(|&(a, b)| open + 1 < b && close > a)
            {
                spans.push((open + 1, close));
            }
            from = close + 1;
        }
    }
    spans.sort_unstable();
    splice(s, &spans, "<project>")
}

fn data_files(s: &str) -> String {
    let mut spans = Vec::new();
    let mut start = None;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if file_word(c) {
            start.get_or_insert(i);
        }
        let last = it.peek().is_none();
        if (!file_word(c) || last)
            && let Some(a) = start.take()
        {
            let b = if file_word(c) { i + c.len_utf8() } else { i };
            let w = s[a..b].trim_end_matches('.');
            if is_data_file(w) {
                spans.push((a, a + w.len()));
            }
        }
    }
    splice(s, &spans, "<project>")
}

/// `s` cut to `max` bytes (at a character), ending with `…` when cut.
pub fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max.saturating_sub('…'.len_utf8());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// The end of `s`, at most `max` bytes, starting with `…` when cut (log
/// lines: the newest matter).
pub fn cut_front(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut start = s.len() - max.saturating_sub('…'.len_utf8());
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &s[start..])
}

/// A code as the contract wants it: lower case letters, digits, `.`, `-`
/// and `_`, at most [`crate::MAX_CODE`] characters.
pub fn clean_code(code: &str) -> String {
    let mut c: String = code
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || ".-_".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    c.truncate(crate::MAX_CODE);
    let c = c.trim_matches(['-', '.', '_']);
    if c.is_empty() {
        "unknown".to_owned()
    } else {
        c.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux() -> Sanitizer {
        let mut s = Sanitizer::new(Some("/home/mrossi"), Some("mrossi"));
        s.redact("BIA-07");
        s.redact("AL-2026-0142");
        s
    }

    #[test]
    fn the_home_folder_and_the_user_go() {
        let s = linux();
        assert_eq!(
            s.clean("log in /home/mrossi/.local/share/dentalcraft/logs"),
            "log in ~/.local/share/dentalcraft/logs"
        );
        // another folder of the same prefix is not the home folder, but it
        // is somebody else's folder: it goes too
        assert_eq!(s.clean("/home/mrossi2/x"), "<project>");
        assert_eq!(s.clean("user mrossi signed out"), "user <user> signed out");
        assert_eq!(s.clean("mrossiX"), "mrossiX");
        // the toolchain's folders stay: they locate the bug
        assert_eq!(
            s.clean("at /home/mrossi/.cargo/registry/src/x/lib.rs:1:2"),
            "at ~/.cargo/registry/src/x/lib.rs:1:2"
        );
    }

    #[test]
    fn projects_scans_and_their_folders_go() {
        let s = linux();
        assert_eq!(
            s.clean("cannot open /home/mrossi/Cases/Mario Rossi/AL-1.dcprj: No such file"),
            "cannot open <project>: No such file"
        );
        assert_eq!(
            s.clean("cannot read upper_jaw.STL (empty)"),
            "cannot read <project> (empty)"
        );
        assert_eq!(
            s.clean(r#"file "BIA 07 lower.ply" skipped"#),
            r#"file "<project>" skipped"#
        );
        assert_eq!(s.clean("saved x.dcprj.tmp."), "saved <project>.");
        assert_eq!(
            s.clean("scan /tmp/dentalcraft-scan-ab12/upper.stl failed"),
            "scan <project>"
        );
        assert_eq!(
            s.clean("D:\\Pazienti\\caso.dcprj: denied"),
            "<project>: denied"
        );
        assert_eq!(
            s.clean("C:\\Program Files\\DentalCraft\\dentalcraft.exe"),
            "C:\\Program Files\\DentalCraft\\dentalcraft.exe"
        );
        // the registered case words, also inside a file stem
        assert_eq!(
            s.clean("case AL-2026-0142_BIA-07 ready"),
            "case <case>_<case> ready"
        );
        // source files of a panic stay
        assert_eq!(
            s.clean("panicked at crates/dc-mesh/src/stl.rs:10:5"),
            "panicked at crates/dc-mesh/src/stl.rs:10:5"
        );
    }

    #[test]
    fn emails_and_tokens_go() {
        let s = linux();
        assert_eq!(
            s.clean("signed in as mario.rossi@studio.it"),
            "signed in as <email>"
        );
        assert_eq!(s.clean("no @ here, a@b"), "no @ here, a@b");
        assert_eq!(
            s.clean("Authorization: Bearer dcat_Zx81kLm0pQ"),
            "Authorization: Bearer <token>"
        );
        assert_eq!(s.clean("bearer abcdefgh12"), "bearer <token>");
        assert_eq!(
            s.clean("refresh dcrt_abcdef123456 failed"),
            "refresh <token> failed"
        );
        assert_eq!(
            s.clean("callback /cb?code=abc123&state=xyz&iss=x"),
            "callback /cb?code=<token>&state=<token>&iss=x"
        );
        assert_eq!(
            s.clean(r#"{"access_token":"dcat_1234567890","x":1}"#),
            r#"{"access_token":"<token>","x":1}"#
        );
        assert_eq!(s.clean("password=hunter2 ok"), "password=<token> ok");
        assert_eq!(
            s.clean("jwt eyJhbGciOiJFZERTQSJ9.eyJzdWIiOiIxIn0.sig"),
            "jwt <token>"
        );
        assert_eq!(
            s.clean("key Qm9vZ2llV29vZ2llMTIzNDU2Nzg5MEFCQ0RFRmdoaWprbG1u"),
            "key <token>"
        );
        // digests and uuids are no secrets: they help to find a blob
        let hex = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
        assert_eq!(s.clean(hex), hex);
        let uuid = "8f0c1d2e-3a4b-4c5d-8e9f-0a1b2c3d4e5f";
        assert_eq!(s.clean(uuid), uuid);
        // a structured log field is not a query
        assert_eq!(
            s.clean(r#"ERROR x: save failed code="save-failed""#),
            r#"ERROR x: save failed code="save-failed""#
        );
    }

    #[test]
    fn windows_paths() {
        let s = Sanitizer::new(Some("C:\\Users\\Anna"), Some("Anna"));
        assert_eq!(
            s.clean("C:\\Users\\Anna\\Documents\\Casi\\x.dcprj: locked"),
            "<project>: locked"
        );
        assert_eq!(
            s.clean("logs in C:\\Users\\Anna\\AppData\\Roaming\\DentalCraft\\data\\logs"),
            "logs in ~\\AppData\\Roaming\\DentalCraft\\data\\logs"
        );
        assert_eq!(s.clean("Anna's file"), "<user>'s file");
    }

    #[test]
    fn every_absolute_path_goes_but_the_allowed_ones() {
        let s = linux();
        // network shares, server folders, other users: no folder name survives
        for (text, want) in [
            (
                // a path may hold spaces: the rest of the line goes with it
                "open /run/user/1000/gvfs/smb-share:server=nas,share=pazienti/Rossi Mario/upper.stl failed",
                "open <project>",
            ),
            (
                r"open \\nas\share\Rossi Mario\upper.stl: denied",
                "open <project>: denied",
            ),
            ("load /srv/cases/Rossi/x.dcprj", "load <project>"),
            ("dir /data/Bianchi Anna/scans", "dir <project>"),
            ("dir /opt/studio/Verdi/archive", "dir <project>"),
            ("dir /Users/Shared/Pazienti/Neri", "dir <project>"),
            ("dir /home/anna/Pazienti/Neri", "dir <project>"),
            ("dir //nas/pazienti/Neri", "dir <project>"),
            (r"dir E:\Studio\Neri Luca", "dir <project>"),
        ] {
            let got = s.clean(text);
            assert_eq!(got, want, "{text}");
            for name in ["Rossi", "Mario", "Bianchi", "Verdi", "Neri", "pazienti"] {
                assert!(!got.contains(name), "{name} left in {got}");
            }
        }
        // the allowed folders keep their path: they locate the bug
        for kept in [
            "/usr/lib64/libvulkan.so.1",
            "/rustc/58cf8f9c35b3/library/std/src/panicking.rs:10:5",
            "/app/bin/dentalcraft",
            "/proc/self/exe",
            "/dev/video0",
            r"C:\Program Files\DentalCraft\dentalcraft.exe",
        ] {
            assert_eq!(s.clean(kept), kept);
        }
        // a dependency built in another home keeps only the toolchain part
        assert_eq!(
            s.clean("at /home/builder/.cargo/registry/src/index/zip-2.1/src/read.rs:9:1"),
            "at <path>/.cargo/registry/src/index/zip-2.1/src/read.rs:9:1"
        );
        // a single word after a slash is no folder (a web route)
        assert_eq!(s.clean("GET /me returned 401"), "GET /me returned 401");
        // URLs are not paths
        assert_eq!(
            s.clean("https://dentalcraft.app/api/v1/diagnostics"),
            "https://dentalcraft.app/api/v1/diagnostics"
        );
    }

    #[test]
    fn serial_numbers_and_mac_addresses_go() {
        let s = linux();
        assert_eq!(
            s.clean("systemConnectCamera failed for MINI2 (RVPM2A2301234567): in use"),
            "systemConnectCamera failed for MINI2 (<serial>): in use"
        );
        assert_eq!(
            s.clean("turntable at 3C:71:BF:0A:12:9e lost"),
            "turntable at <serial> lost"
        );
        assert_eq!(s.clean("nic 3c-71-bf-0a-12-9e"), "nic <serial>");
        assert_eq!(
            s.clean("device_id=mini2-0042 busy"),
            "device_id=<serial> busy"
        );
        assert_eq!(
            s.clean(r#"{"serial":"x1y2z3","fw":"1.2"}"#),
            r#"{"serial":"<serial>","fw":"1.2"}"#
        );
        // what helps a bug report stays: times, versions, short codes
        for kept in [
            "at 12:30:45",
            "version 0.2.0-alpha.15",
            "GPU RTX4090",
            "E0433",
        ] {
            assert_eq!(s.clean(kept), kept);
        }
    }

    #[test]
    fn short_words_and_cuts() {
        let mut s = Sanitizer::new(None, Some("x"));
        s.redact("AB");
        assert_eq!(s.clean("AB x"), "AB x");
        assert_eq!(cut("abcdef", 10), "abcdef");
        let c = cut("àèìòù àèìòù", 8);
        assert!(c.len() <= 8 && c.ends_with('…'), "{c}");
        let f = cut_front("line 1\nline 2\nline 3", 10);
        assert!(
            f.len() <= 10 && f.starts_with('…') && f.ends_with("line 3"),
            "{f}"
        );
        assert_eq!(clean_code("Save Failed!"), "save-failed");
        assert_eq!(clean_code("scan.device"), "scan.device");
        assert_eq!(clean_code("   "), "unknown");
        assert_eq!(clean_code(&"a".repeat(100)).len(), crate::MAX_CODE);
    }
}
