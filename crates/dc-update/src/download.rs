//! HTTP (`ureq`, rustls with ring): the manifest with its signature, and
//! the downloads, checked against the size and SHA-256 the signed manifest
//! announces.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::manifest::check_url;
use crate::{Asset, UpdateError};

/// Largest manifest or signature accepted.
const MAX_SMALL: u64 = 256 * 1024;
const USER_AGENT: &str = concat!("DentalCraft/", env!("CARGO_PKG_VERSION"));

fn transport(e: &ureq::Error) -> UpdateError {
    UpdateError::Offline(e.to_string())
}

/// A small file (the manifest, its signature) in one request of at most
/// 20 seconds.
pub(crate) fn get_small(url: &str) -> Result<Vec<u8>, UpdateError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .header("Cache-Control", "no-cache")
        .call()
        .map_err(|e| transport(&e))?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(UpdateError::Server(format!("{url}: HTTP {status}")));
    }
    resp.body_mut()
        .with_config()
        .limit(MAX_SMALL)
        .read_to_vec()
        .map_err(|e| UpdateError::Server(format!("{url}: {e}")))
}

/// SHA-256 of `bytes`, lowercase hexadecimal.
pub(crate) fn hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The `.part` file next to `dest` that a download writes first.
pub(crate) fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// Downloads `asset` to `dest`: written to `dest.part`, hashed on the way,
/// renamed to `dest` only when its size and SHA-256 are the announced ones
/// (else deleted: [`UpdateError::Damaged`]). `progress(done, total)` follows
/// it; `cancel` stops it ([`UpdateError::Cancelled`]).
pub(crate) fn download(
    asset: &Asset,
    dest: &Path,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<(), UpdateError> {
    check_url(&asset.url).map_err(UpdateError::Server)?;
    let part = part_path(dest);
    let result = fetch(asset, &part, progress, cancel);
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
        return result;
    }
    std::fs::rename(&part, dest).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        UpdateError::Install(format!("{}: {e}", dest.display()))
    })
}

fn fetch(
    asset: &Asset,
    part: &Path,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<(), UpdateError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        // a stalled transfer fails; a slow one may take as long as it needs
        .timeout_per_read(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .build()
        .into();
    let mut resp = agent.get(&asset.url).call().map_err(|e| transport(&e))?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(UpdateError::Server(format!("{}: HTTP {status}", asset.url)));
    }
    let mut file =
        File::create(part).map_err(|e| UpdateError::Install(format!("{}: {e}", part.display())))?;
    // one byte over the announced size is enough to know it is wrong (the
    // limit reader also fails a read once the limit is used up)
    let mut body = resp
        .body_mut()
        .with_config()
        .limit(asset.size.saturating_add(1))
        .reader();
    let mut hasher = Sha256::new();
    let mut buf = vec![0_u8; 64 * 1024];
    let mut done = 0_u64;
    progress(0, asset.size);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(UpdateError::Cancelled);
        }
        let n = match body.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                // over the limit: more bytes than announced
                if done >= asset.size {
                    return Err(UpdateError::Damaged);
                }
                return Err(UpdateError::Offline(e.to_string()));
            }
        };
        done += n as u64;
        if done > asset.size {
            return Err(UpdateError::Damaged);
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| UpdateError::Install(format!("{}: {e}", part.display())))?;
        progress(done, asset.size);
    }
    file.sync_all()
        .map_err(|e| UpdateError::Install(format!("{}: {e}", part.display())))?;
    let sha = hex(&hasher.finalize());
    if done != asset.size || !sha.eq_ignore_ascii_case(asset.sha256.trim()) {
        tracing::warn!(
            "{}: {done} bytes, SHA-256 {sha}; announced {} bytes, {}",
            asset.url,
            asset.size,
            asset.sha256
        );
        return Err(UpdateError::Damaged);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    use super::*;

    /// A tiny HTTP server on 127.0.0.1 answering `GET /<name>` from `files`
    /// (404 otherwise), one request per connection, until the test ends.
    pub(crate) fn serve(files: HashMap<String, Vec<u8>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                // the headers, up to the blank line
                let mut header = String::new();
                while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
                    header.clear();
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/");
                let mut out = stream;
                let _ = match files.get(path.trim_start_matches('/')) {
                    Some(body) => out
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            )
                            .as_bytes(),
                        )
                        .and_then(|()| out.write_all(body)),
                    None => out.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    ),
                };
            }
        });
        base
    }

    pub(crate) fn asset_for(base: &str, name: &str, body: &[u8]) -> Asset {
        Asset {
            url: format!("{base}/{name}"),
            sha256: hex(&Sha256::digest(body)),
            size: body.len() as u64,
        }
    }

    fn files(items: &[(&str, &[u8])]) -> HashMap<String, Vec<u8>> {
        items
            .iter()
            .map(|(n, b)| ((*n).to_owned(), b.to_vec()))
            .collect()
    }

    #[test]
    fn a_download_with_the_announced_size_and_content_is_kept() {
        let body = vec![7_u8; 200_000];
        let base = serve(files(&[("a.AppImage", &body)]));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a.AppImage");
        let seen = Arc::new(AtomicU64::new(0));
        let s = Arc::clone(&seen);
        download(
            &asset_for(&base, "a.AppImage", &body),
            &dest,
            &move |done, total| {
                assert_eq!(total, 200_000);
                s.store(done, Ordering::Relaxed);
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(seen.load(Ordering::Relaxed), 200_000);
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn a_download_with_other_content_is_refused_and_deleted() {
        let body = b"the real file".to_vec();
        let base = serve(files(&[("a.zip", b"a fake file!!")]));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a.zip");
        let mut asset = asset_for(&base, "a.zip", &body);
        let r = download(&asset, &dest, &|_, _| {}, &AtomicBool::new(false));
        assert_eq!(r, Err(UpdateError::Damaged));
        assert!(!dest.exists() && !part_path(&dest).exists());
        // longer and shorter than announced
        for size in [5, 50] {
            asset.size = size;
            let r = download(&asset, &dest, &|_, _| {}, &AtomicBool::new(false));
            assert_eq!(r, Err(UpdateError::Damaged), "size {size}");
            assert!(!dest.exists() && !part_path(&dest).exists());
        }
    }

    #[test]
    fn a_cancelled_download_leaves_nothing() {
        let body = vec![1_u8; 100_000];
        let base = serve(files(&[("a.exe", &body)]));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a.exe");
        let r = download(
            &asset_for(&base, "a.exe", &body),
            &dest,
            &|_, _| {},
            &AtomicBool::new(true),
        );
        assert_eq!(r, Err(UpdateError::Cancelled));
        assert!(!dest.exists() && !part_path(&dest).exists());
    }

    #[test]
    fn missing_files_and_closed_ports_are_reported() {
        let base = serve(HashMap::new());
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("x.msi");
        let r = download(
            &asset_for(&base, "x.msi", b"x"),
            &dest,
            &|_, _| {},
            &AtomicBool::new(false),
        );
        assert!(matches!(r, Err(UpdateError::Server(_))), "{r:?}");
        assert!(matches!(
            get_small(&format!("{base}/update.json")),
            Err(UpdateError::Server(_))
        ));
        // port 9 (discard) on loopback is closed on CI and dev machines
        assert!(matches!(
            get_small("http://127.0.0.1:9/update.json"),
            Err(UpdateError::Offline(_))
        ));
        // never plain http to another computer
        let mut remote = asset_for("http://dentalcraft.app", "x.msi", b"x");
        assert!(matches!(
            download(&remote, &dest, &|_, _| {}, &AtomicBool::new(false)),
            Err(UpdateError::Server(_))
        ));
        remote.url = "file:///etc/passwd".into();
        assert!(matches!(
            download(&remote, &dest, &|_, _| {}, &AtomicBool::new(false)),
            Err(UpdateError::Server(_))
        ));
    }
}
