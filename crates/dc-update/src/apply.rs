//! Installing a newer version: [`prepare`] downloads and checks it and puts
//! it in place next to the installation, [`Prepared::finish`] switches to it
//! (when the program closes) and can start it.
//!
//! | installation | prepare | finish |
//! |---|---|---|
//! | standalone (`.tar.gz`, `.zip`) | unpacks the archive into `<folder>/.update/staged/` | moves each unpacked entry into the folder, the one it replaces into `<folder>/.update/previous/` (renames only, undone on failure); a running program may be renamed on Linux and Windows |
//! | AppImage | downloads next to `$APPIMAGE` (`.<name>.new`) | renames it over `$APPIMAGE` (atomic) |
//! | macOS `DentalCraft.app` (`.zip`) | unpacks the archive into `<folder of the app>/.<name>.app.update/staged/` (same volume as the app) | renames the app into `previous/` of that folder and the unpacked one to its place (two renames, undone on failure); a running app may be renamed on macOS; restart opens the new one with `open -n` |
//! | Windows setup `.exe` / `.msi` | downloads into the work folder | starts a hidden PowerShell that waits for this process to end, then runs `setup.exe /S /D=<folder>` or `msiexec /i <file> /passive`, then (restart) the installed `dentalcraft.exe` |
//!
//! [`cleanup`] removes, at the next start, what the update left behind.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::download::{download, part_path};
use crate::install::APP_BIN;
use crate::{Action, Asset, AssetKey, Check, InstallKind, Installation, UpdateError};

/// Folder inside a standalone installation that holds the unpacked update
/// (`staged`) and the replaced version (`previous`, removed at the next
/// start).
pub const UPDATE_DIR: &str = ".update";
const STAGED: &str = "staged";
const PREVIOUS: &str = "previous";

/// A downloaded and checked update, ready to replace the installation.
#[derive(Debug)]
pub struct Prepared {
    /// The version it installs.
    pub version: String,
    step: Step,
}

#[derive(Debug)]
enum Step {
    /// Standalone: move the entries of `staged` into `dir`.
    Swap { dir: PathBuf, staged: PathBuf },
    /// macOS: rename the `staged` bundle (unpacked in `update`, a folder
    /// next to the installed one) to the place of `bundle`.
    Bundle {
        bundle: PathBuf,
        staged: PathBuf,
        update: PathBuf,
    },
    /// AppImage: rename `new` over `target`.
    Replace { new: PathBuf, target: PathBuf },
    /// Windows installer to run once this process has ended.
    Installer {
        file: PathBuf,
        msi: bool,
        dir: PathBuf,
    },
}

impl Prepared {
    /// A standalone update already unpacked in `staged`, for the folder
    /// `dir` (tests, tools).
    pub fn swap(version: &str, dir: impl Into<PathBuf>, staged: impl Into<PathBuf>) -> Self {
        Self {
            version: version.to_owned(),
            step: Step::Swap {
                dir: dir.into(),
                staged: staged.into(),
            },
        }
    }

    /// A macOS bundle update already unpacked as `staged` in `update`, for
    /// the installed `bundle` (tests, tools).
    pub fn bundle(
        version: &str,
        bundle: impl Into<PathBuf>,
        staged: impl Into<PathBuf>,
        update: impl Into<PathBuf>,
    ) -> Self {
        Self {
            version: version.to_owned(),
            step: Step::Bundle {
                bundle: bundle.into(),
                staged: staged.into(),
                update: update.into(),
            },
        }
    }

    /// `true` when it is installed by a separate installer once this
    /// process has ended (Windows setup and `.msi`).
    pub fn runs_installer(&self) -> bool {
        matches!(self.step, Step::Installer { .. })
    }

    /// Switches to the new version; with `restart`, starts it. Call it when
    /// the program closes (the project is saved). A failed switch leaves
    /// the installed version as it was.
    pub fn finish(self, restart: bool) -> Result<(), UpdateError> {
        match self.step {
            Step::Swap { dir, staged } => {
                swap(&dir, &staged)
                    .map_err(|e| UpdateError::Install(format!("{}: {e}", dir.display())))?;
                let _ = fs::remove_dir(&staged);
                tracing::info!("updated {} to {}", dir.display(), self.version);
                if restart {
                    launch(&dir.join(APP_BIN))?;
                }
            }
            Step::Bundle {
                bundle,
                staged,
                update,
            } => {
                swap_bundle(&bundle, &staged, &update)
                    .map_err(|e| UpdateError::Install(format!("{}: {e}", bundle.display())))?;
                let _ = fs::remove_dir(update.join(STAGED));
                tracing::info!("updated {} to {}", bundle.display(), self.version);
                if restart {
                    open_bundle(&bundle)?;
                }
            }
            Step::Replace { new, target } => {
                fs::rename(&new, &target)
                    .map_err(|e| UpdateError::Install(format!("{}: {e}", target.display())))?;
                tracing::info!("updated {} to {}", target.display(), self.version);
                if restart {
                    launch(&target)?;
                }
            }
            Step::Installer { file, msi, dir } => {
                run_installer(&installer_script(
                    std::process::id(),
                    &file,
                    msi,
                    &dir,
                    restart,
                ))?;
                tracing::info!("{} runs once DentalCraft has closed", file.display());
            }
        }
        Ok(())
    }

    /// Drops the update without installing it: the unpacked or downloaded
    /// files are deleted.
    pub fn discard(self) {
        let _ = match self.step {
            Step::Swap { staged, .. } => fs::remove_dir_all(staged),
            Step::Bundle { update, .. } => fs::remove_dir_all(update.join(STAGED)),
            Step::Replace { new, .. } => fs::remove_file(new),
            Step::Installer { file, .. } => fs::remove_file(file),
        };
    }
}

/// Downloads, checks and puts in place the update of `check` (blocking:
/// call it from a worker thread). `work_dir` holds the downloads (archives,
/// installers); `progress(done, total)` follows the download, `cancel`
/// stops it. Nothing of the installation changes before
/// [`Prepared::finish`].
pub fn prepare(
    check: &Check,
    work_dir: &Path,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<Prepared, UpdateError> {
    let Action::Install { key, asset } = &check.action else {
        return Err(UpdateError::NoDownload);
    };
    let inst = &check.installation;
    let step = match inst.kind {
        InstallKind::Standalone => {
            prepare_standalone(inst, *key, asset, work_dir, progress, cancel)?
        }
        InstallKind::MacApp => prepare_bundle(inst, asset, work_dir, progress, cancel)?,
        InstallKind::AppImage => {
            let target = inst.appimage.as_deref().ok_or(UpdateError::NoDownload)?;
            prepare_appimage(target, asset, progress, cancel)?
        }
        InstallKind::Exe | InstallKind::Msi => {
            let file = download_into(work_dir, asset, progress, cancel)?;
            Step::Installer {
                file,
                msi: inst.kind == InstallKind::Msi,
                dir: inst.dir.clone(),
            }
        }
        _ => return Err(UpdateError::NoDownload),
    };
    Ok(Prepared {
        version: check.manifest.version.clone(),
        step,
    })
}

/// Removes what an earlier update left behind (errors ignored: a file still
/// in use goes the next time): the replaced version of a standalone
/// installation, the AppImage being downloaded when the program stopped,
/// and the downloads in `work_dir`.
pub fn cleanup(installation: &Installation, work_dir: &Path) {
    if installation.kind == InstallKind::Standalone {
        clear_previous(&installation.dir.join(UPDATE_DIR));
    }
    if installation.kind == InstallKind::MacApp
        && let Some(update) = installation.bundle().and_then(|b| bundle_update_dir(&b))
    {
        clear_previous(&update);
    }
    if let Some(target) = &installation.appimage {
        let new = appimage_download(target);
        let _ = fs::remove_file(part_path(&new));
        let _ = fs::remove_file(new);
    }
    let _ = fs::remove_dir_all(work_dir);
}

/// Removes the `previous*` folders of an update folder, then the folder
/// itself when nothing else is in it.
fn clear_previous(update: &Path) {
    for entry in fs::read_dir(update).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with(PREVIOUS) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    let _ = fs::remove_dir(update);
}

/// `dir` exists and a file can be created in it; `shown` is the folder the
/// error names.
fn writable(dir: &Path, shown: &Path) -> Result<(), UpdateError> {
    let fail = |e: io::Error| {
        tracing::warn!("cannot write to {}: {e}", dir.display());
        UpdateError::NotWritable(shown.to_path_buf())
    };
    fs::create_dir_all(dir).map_err(fail)?;
    let probe = dir.join(format!(".write-test-{}", std::process::id()));
    File::create(&probe).map_err(fail)?;
    let _ = fs::remove_file(probe);
    Ok(())
}

/// Downloads `asset` into `work_dir` under its own name.
fn download_into(
    work_dir: &Path,
    asset: &Asset,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<PathBuf, UpdateError> {
    writable(work_dir, work_dir)?;
    let dest = work_dir.join(asset.file_name());
    download(asset, &dest, progress, cancel)?;
    Ok(dest)
}

fn prepare_standalone(
    inst: &Installation,
    key: AssetKey,
    asset: &Asset,
    work_dir: &Path,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<Step, UpdateError> {
    let dir = &inst.dir;
    let update = dir.join(UPDATE_DIR);
    // the folder must take the new files: known before downloading
    writable(&update, dir)?;
    let archive = download_into(work_dir, asset, progress, cancel)?;
    let staged = update.join(STAGED);
    let _ = fs::remove_dir_all(&staged);
    let unpacked = fs::create_dir_all(&staged)
        .map_err(|e| UpdateError::Install(format!("{}: {e}", staged.display())))
        .and_then(|()| match key {
            AssetKey::LinuxStandalone => extract_tar_gz(&archive, &staged),
            AssetKey::WindowsStandalone => extract_zip(&archive, &staged),
            _ => Err(UpdateError::NoDownload),
        })
        .and_then(|()| {
            if staged.join(APP_BIN).is_file() {
                Ok(())
            } else {
                Err(UpdateError::Install(format!(
                    "{} has no {APP_BIN}",
                    asset.file_name()
                )))
            }
        });
    let _ = fs::remove_file(&archive);
    if let Err(e) = unpacked {
        let _ = fs::remove_dir_all(&staged);
        return Err(e);
    }
    Ok(Step::Swap {
        dir: dir.clone(),
        staged,
    })
}

/// The folder that holds the unpacked update and the replaced version of
/// the installed macOS `bundle`: `.<name>.update` next to it (the same
/// volume, so the renames are atomic).
fn bundle_update_dir(bundle: &Path) -> Option<PathBuf> {
    let mut name = OsString::from(".");
    name.push(bundle.file_name()?);
    name.push(".update");
    Some(bundle.with_file_name(name))
}

fn prepare_bundle(
    inst: &Installation,
    asset: &Asset,
    work_dir: &Path,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<Step, UpdateError> {
    let bundle = inst.bundle().ok_or(UpdateError::NoDownload)?;
    let update = bundle_update_dir(&bundle).ok_or(UpdateError::NoDownload)?;
    let parent = bundle.parent().ok_or(UpdateError::NoDownload)?;
    // the folder of the app must take the new bundle: known before
    // downloading (a translocated or read-only app fails here, with the
    // advice to move it to /Applications)
    writable(&update, parent)?;
    let archive = download_into(work_dir, asset, progress, cancel)?;
    let root = update.join(STAGED);
    let _ = fs::remove_dir_all(&root);
    let unpacked = fs::create_dir_all(&root)
        .map_err(|e| UpdateError::Install(format!("{}: {e}", root.display())))
        .and_then(|()| extract_app_zip(&archive, &root));
    let _ = fs::remove_file(&archive);
    let staged = match unpacked {
        Ok(staged) => staged,
        Err(e) => {
            let _ = fs::remove_dir_all(&root);
            return Err(e);
        }
    };
    if !staged
        .join("Contents")
        .join("MacOS")
        .join(APP_BIN)
        .is_file()
    {
        let _ = fs::remove_dir_all(&root);
        return Err(UpdateError::Install(format!(
            "{} has no {APP_BIN}",
            asset.file_name()
        )));
    }
    // Only now, on the bundle whose size and SHA-256 matched the signed
    // announcement: nothing a browser or the system marked is in it, but
    // a quarantine flag would make Gatekeeper refuse the ad-hoc signature.
    if cfg!(target_os = "macos") {
        strip_quarantine(&staged);
        // the unpacked bundle must be whole (every file and permission the
        // seal covers) or macOS would refuse to start it after the swap
        if let Err(e) = verify_signature(&staged) {
            let _ = fs::remove_dir_all(&root);
            return Err(e);
        }
    }
    Ok(Step::Bundle {
        bundle,
        staged,
        update,
    })
}

/// Removes the `com.apple.quarantine` attribute from a bundle that was
/// verified (errors only logged: it is absent on our own downloads).
fn strip_quarantine(bundle: &Path) {
    let out = std::process::Command::new("/usr/bin/xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(bundle)
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {}
        Ok(o) => tracing::debug!(
            "xattr on {}: {}",
            bundle.display(),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => tracing::debug!("xattr on {}: {e}", bundle.display()),
    }
}

/// `codesign --verify --deep --strict` on the unpacked `bundle`: the seal
/// (ad-hoc today) covers the programs, `Info.plist` and the resources.
fn verify_signature(bundle: &Path) -> Result<(), UpdateError> {
    let out = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(bundle)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| UpdateError::Install(format!("codesign: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    Err(UpdateError::Install(format!(
        "the unpacked app is damaged: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    )))
}

/// Puts `staged` in the place of `bundle`: the installed bundle goes into a
/// new `previous` folder of `update`, then `staged` is renamed to `bundle`.
/// On failure the installed bundle is back where it was.
pub(crate) fn swap_bundle(bundle: &Path, staged: &Path, update: &Path) -> io::Result<()> {
    let name = bundle
        .file_name()
        .ok_or_else(|| io::Error::other("the bundle has no name"))?;
    if bundle.symlink_metadata().is_err() {
        // the app was deleted by hand: the new one is still put in place
        return fs::rename(staged, bundle);
    }
    let previous = new_dir(update, PREVIOUS)?;
    let aside = previous.join(name);
    fs::rename(bundle, &aside).inspect_err(|_| {
        let _ = fs::remove_dir(&previous);
    })?;
    fs::rename(staged, bundle).inspect_err(|_| {
        let _ = fs::rename(&aside, bundle);
        let _ = fs::remove_dir(&previous);
    })
}

/// Opens a new instance of the app `bundle` through Launch Services (which
/// gives it its own privacy permissions and a normal app environment).
fn open_bundle(bundle: &Path) -> Result<(), UpdateError> {
    let fail =
        |e: &dyn std::fmt::Display| UpdateError::Install(format!("open {}: {e}", bundle.display()));
    let status = std::process::Command::new("/usr/bin/open")
        .arg("-n")
        .arg(bundle)
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|e| fail(&e))?;
    if !status.success() {
        return Err(fail(&status));
    }
    tracing::info!("opened {}", bundle.display());
    Ok(())
}

/// Where the new AppImage is downloaded: next to the old one (same file
/// system, so the final rename is atomic), hidden.
fn appimage_download(target: &Path) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(target.file_name().unwrap_or_default());
    name.push(".new");
    target.with_file_name(name)
}

fn prepare_appimage(
    target: &Path,
    asset: &Asset,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<Step, UpdateError> {
    let dir = target.parent().unwrap_or(Path::new("."));
    writable(dir, dir)?;
    let new = appimage_download(target);
    download(asset, &new, progress, cancel)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(target).map_or(0o755, |m| m.permissions().mode()) | 0o755;
        if let Err(e) = fs::set_permissions(&new, fs::Permissions::from_mode(mode & 0o7777)) {
            let _ = fs::remove_file(&new);
            return Err(UpdateError::Install(format!("{}: {e}", new.display())));
        }
    }
    Ok(Step::Replace {
        new,
        target: target.to_path_buf(),
    })
}

/// Starts `program` (the updated app), detached from this process.
fn launch(program: &Path) -> Result<(), UpdateError> {
    let mut cmd = std::process::Command::new(program);
    if let Some(dir) = program.parent() {
        cmd.current_dir(dir);
    }
    let child = cmd
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| UpdateError::Install(format!("{}: {e}", program.display())))?;
    tracing::info!("started {} (process {})", program.display(), child.id());
    Ok(())
}

// ---------------------------------------------------------------- standalone

/// Moves every entry of `staged` into `dir`. An entry it replaces goes into a
/// new `previous` folder in `dir/.update/`. On failure, what was moved goes
/// back: the installation is as it was.
pub(crate) fn swap(dir: &Path, staged: &Path) -> io::Result<()> {
    let previous = new_dir(&dir.join(UPDATE_DIR), PREVIOUS)?;
    swap_into(dir, staged, &previous)
}

/// A new, empty folder `parent/stem`, or `parent/stem-N` when that exists
/// (a previous version not removed yet, its program still running).
fn new_dir(parent: &Path, stem: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(parent)?;
    for n in 0..1000 {
        let p = match n {
            0 => parent.join(stem),
            n => parent.join(format!("{stem}-{n}")),
        };
        match fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "{}: too many {stem} folders",
        parent.display()
    )))
}

fn swap_into(dir: &Path, staged: &Path, previous: &Path) -> io::Result<()> {
    let mut names = fs::read_dir(staged)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<io::Result<Vec<OsString>>>()?;
    names.sort();
    // (entry, it replaced an entry of the installation)
    let mut moved: Vec<(OsString, bool)> = Vec::new();
    for name in names {
        let target = dir.join(&name);
        let old = target.symlink_metadata().is_ok();
        let step = (|| {
            if old {
                fs::rename(&target, previous.join(&name))?;
            }
            fs::rename(staged.join(&name), &target).inspect_err(|_| {
                if old {
                    let _ = fs::rename(previous.join(&name), &target);
                }
            })
        })();
        if let Err(e) = step {
            for (name, old) in moved.iter().rev() {
                let _ = fs::rename(dir.join(name), staged.join(name));
                if *old {
                    let _ = fs::rename(previous.join(name), dir.join(name));
                }
            }
            return Err(e);
        }
        moved.push((name, old));
    }
    Ok(())
}

/// The path of an archive entry inside the single top folder of the
/// archive (`dentalcraft_<version>_<target>/`): `None` for the folder
/// itself. Refuses absolute paths, `..`, a second top folder and the
/// [`UPDATE_DIR`] name.
fn inside_top(path: &Path, top: &mut Option<OsString>) -> Result<Option<PathBuf>, UpdateError> {
    let unsafe_path = || {
        UpdateError::Install(format!(
            "unexpected path in the archive: {}",
            path.display()
        ))
    };
    let mut parts = path.components().filter(|c| *c != Component::CurDir);
    let Some(Component::Normal(first)) = parts.next() else {
        return Err(unsafe_path());
    };
    match top {
        Some(t) if t.as_os_str() != first => return Err(unsafe_path()),
        Some(_) => {}
        None => *top = Some(first.to_os_string()),
    }
    let mut rel = PathBuf::new();
    for part in parts {
        match part {
            Component::Normal(p) if !(rel.as_os_str().is_empty() && p == UPDATE_DIR) => rel.push(p),
            _ => return Err(unsafe_path()),
        }
    }
    Ok((!rel.as_os_str().is_empty()).then_some(rel))
}

fn extract_tar_gz(archive: &Path, into: &Path) -> Result<(), UpdateError> {
    use tar::EntryType;
    let io = |e: io::Error| UpdateError::Install(format!("{}: {e}", archive.display()));
    let file = File::open(archive).map_err(io)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(BufReader::new(file)));
    let mut top = None;
    for entry in tar.entries().map_err(io)? {
        let mut entry = entry.map_err(io)?;
        let path = entry.path().map_err(io)?.into_owned();
        let kind = entry.header().entry_type();
        if matches!(
            kind,
            EntryType::XGlobalHeader | EntryType::XHeader | EntryType::GNULongName
        ) {
            continue;
        }
        let Some(rel) = inside_top(&path, &mut top)? else {
            continue;
        };
        let dest = into.join(&rel);
        match kind {
            EntryType::Directory => fs::create_dir_all(&dest).map_err(io)?,
            EntryType::Regular | EntryType::Continuous => {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(io)?;
                }
                entry.unpack(&dest).map_err(io)?;
            }
            other => {
                return Err(UpdateError::Install(format!(
                    "{}: unexpected {other:?} entry in the archive",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

fn extract_zip(archive: &Path, into: &Path) -> Result<(), UpdateError> {
    extract_zip_with(archive, into, inside_top)
}

/// Unpacks the `.zip` of a macOS bundle (`ditto -c -k --keepParent`) into
/// `into`, which then holds the one `<name>.app` folder; returns its path.
/// The `__MACOSX` folder and `._` files that Finder and ditto may add for
/// extended attributes are skipped.
fn extract_app_zip(archive: &Path, into: &Path) -> Result<PathBuf, UpdateError> {
    let mut app = None;
    extract_zip_with(archive, into, |path, top| {
        let r = inside_app(path, top);
        if let Ok(Some(_)) = &r {
            app = top.clone();
        }
        r
    })?;
    app.map(|name| into.join(name))
        .ok_or_else(|| UpdateError::Install(format!("{} has no .app folder", archive.display())))
}

/// The path of an archive entry of a bundle archive: `None` for what is
/// skipped (see [`extract_app_zip`]). Refuses absolute paths, `..`, and a
/// top folder that is not one `<name>.app`.
fn inside_app(path: &Path, top: &mut Option<OsString>) -> Result<Option<PathBuf>, UpdateError> {
    let unsafe_path = || {
        UpdateError::Install(format!(
            "unexpected path in the archive: {}",
            path.display()
        ))
    };
    let mut parts = path.components().filter(|c| *c != Component::CurDir);
    let Some(Component::Normal(first)) = parts.next() else {
        return Err(unsafe_path());
    };
    if first == "__MACOSX" {
        return Ok(None);
    }
    let named_app = Path::new(first).extension().is_some_and(|e| e == "app");
    match top {
        Some(t) if t.as_os_str() != first => return Err(unsafe_path()),
        Some(_) => {}
        None if named_app => *top = Some(first.to_os_string()),
        None => return Err(unsafe_path()),
    }
    let mut rel = PathBuf::from(first);
    for part in parts {
        match part {
            Component::Normal(p) => rel.push(p),
            _ => return Err(unsafe_path()),
        }
    }
    let apple_double = rel
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with("._"));
    Ok((!apple_double).then_some(rel))
}

fn extract_zip_with(
    archive: &Path,
    into: &Path,
    mut inside: impl FnMut(&Path, &mut Option<OsString>) -> Result<Option<PathBuf>, UpdateError>,
) -> Result<(), UpdateError> {
    let fail =
        |e: &dyn std::fmt::Display| UpdateError::Install(format!("{}: {e}", archive.display()));
    let file = File::open(archive).map_err(|e| fail(&e))?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| fail(&e))?;
    let mut top = None;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| fail(&e))?;
        let name = f.enclosed_name().ok_or_else(|| {
            UpdateError::Install(format!("unexpected path in the archive: {:?}", f.name()))
        })?;
        let Some(rel) = inside(&name, &mut top)? else {
            continue;
        };
        let dest = into.join(&rel);
        if f.is_dir() {
            fs::create_dir_all(&dest).map_err(|e| fail(&e))?;
        } else if f.is_file() {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| fail(&e))?;
            }
            let mut out = File::create(&dest).map_err(|e| fail(&e))?;
            io::copy(&mut f, &mut out).map_err(|e| fail(&e))?;
            #[cfg(unix)]
            if let Some(mode) = f.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dest, fs::Permissions::from_mode(mode & 0o777))
                    .map_err(|e| fail(&e))?;
            }
        } else {
            return Err(UpdateError::Install(format!(
                "{}: unexpected link in the archive",
                rel.display()
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- Windows installers

/// The PowerShell commands that wait for process `pid` to end, then run the
/// installer `file` (`setup.exe /S /D=<dir>`, or `msiexec /i <file>
/// /passive`), then, with `restart` and when it succeeded, start
/// `<dir>\dentalcraft.exe`.
pub(crate) fn installer_script(
    pid: u32,
    file: &Path,
    msi: bool,
    dir: &Path,
    restart: bool,
) -> String {
    // a single-quoted PowerShell string: a quote inside is doubled
    let quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let file = file.display().to_string();
    let dir = dir.display().to_string();
    let run = if msi {
        // msiexec reads the path in double quotes (it may hold spaces)
        format!(
            "$p = Start-Process -FilePath 'msiexec.exe' -ArgumentList @('/i', {}, '/passive', '/norestart') -Wait -PassThru",
            quote(&format!("\"{file}\""))
        )
    } else {
        // NSIS: /D= is the last argument and is never quoted
        format!(
            "$p = Start-Process -FilePath {} -ArgumentList @('/S', {}) -Wait -PassThru",
            quote(&file),
            quote(&format!("/D={dir}"))
        )
    };
    let mut script = format!("Wait-Process -Id {pid} -ErrorAction SilentlyContinue; {run}");
    if restart {
        let app = format!("{}\\dentalcraft.exe", dir.trim_end_matches(['\\', '/']));
        script.push_str(&format!(
            "; if ($p.ExitCode -eq 0 -or $p.ExitCode -eq 3010) {{ Start-Process -FilePath {} }}",
            quote(&app)
        ));
    }
    script
}

/// `script` for `powershell -EncodedCommand`: UTF-16LE, base64 (no quoting
/// rules to get wrong).
pub(crate) fn encoded_command(script: &str) -> String {
    use base64::Engine;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(utf16)
}

/// Starts the hidden PowerShell that runs `script` once this process ends.
fn run_installer(script: &str) -> Result<(), UpdateError> {
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-WindowStyle",
        "Hidden",
        "-EncodedCommand",
    ])
    .arg(encoded_command(script));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // no console window; it outlives this process
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    let child = cmd
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| UpdateError::Install(format!("powershell.exe: {e}")))?;
    tracing::info!("installer helper started (process {})", child.id());
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;

    use sha2::{Digest, Sha256};

    use super::*;
    use crate::Manifest;
    use crate::download::tests::{asset_for, serve};

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    /// `files` as a `.tar.gz` with the top folder `top/`.
    fn tar_gz(top: &str, files: &[(&str, &str)]) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut dir = tar::Header::new_gnu();
        dir.set_entry_type(tar::EntryType::Directory);
        dir.set_mode(0o755);
        dir.set_size(0);
        tar.append_data(&mut dir, format!("{top}/"), io::empty())
            .unwrap();
        for (name, body) in files {
            let mut h = tar::Header::new_gnu();
            h.set_mode(if name.ends_with(APP_BIN) {
                0o755
            } else {
                0o644
            });
            h.set_size(body.len() as u64);
            tar.append_data(&mut h, format!("{top}/{name}"), body.as_bytes())
                .unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    /// `files` as a `.zip` with the top folder `top/`.
    fn zip_file(top: &str, files: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        zip.add_directory(format!("{top}/"), opts).unwrap();
        for (name, body) in files {
            zip.start_file(format!("{top}/{name}"), opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn check_for(kind: InstallKind, dir: &Path, key: AssetKey, asset: Asset) -> Check {
        let mut assets = std::collections::BTreeMap::new();
        assets.insert(key.name().to_owned(), asset);
        let manifest = Manifest {
            version: "999.0.0".into(),
            tag: "v999.0.0".into(),
            date: String::new(),
            notes_url: String::new(),
            assets,
        };
        let mut c = Check::new("0.1.0", manifest, Installation::new(kind, dir));
        // the action of that kind on any test system
        let asset = c.manifest.asset(key).unwrap().clone();
        c.action = Action::Install { key, asset };
        c
    }

    /// An installed standalone folder: the program, its README, a user file.
    fn installed(root: &Path) -> PathBuf {
        let dir = root.join("dentalcraft_0.1.0_linux_x86_64");
        write(&dir.join(APP_BIN), "old app");
        write(&dir.join("README.md"), "old readme");
        write(&dir.join("LICENSES/OFL.txt"), "old licence");
        write(&dir.join("notes-of-the-user.txt"), "mine");
        dir
    }

    fn assert_updated(dir: &Path) {
        assert_eq!(read(&dir.join(APP_BIN)), "new app");
        assert_eq!(read(&dir.join("README.md")), "new readme");
        assert_eq!(read(&dir.join("LICENSES/OFL.txt")), "new licence");
        assert_eq!(read(&dir.join("install-kind")), "standalone\n");
        // files that are not part of DentalCraft stay
        assert_eq!(read(&dir.join("notes-of-the-user.txt")), "mine");
    }

    const NEW_FILES: [(&str, &str); 4] = [
        (APP_BIN, "new app"),
        ("README.md", "new readme"),
        ("LICENSES/OFL.txt", "new licence"),
        ("install-kind", "standalone\n"),
    ];

    #[test]
    fn a_standalone_folder_is_unpacked_aside_then_swapped() {
        let root = tempfile::tempdir().unwrap();
        let dir = installed(root.path());
        let archive = tar_gz("dentalcraft_999.0.0_linux_x86_64", &NEW_FILES);
        let name = "dentalcraft_999.0.0_linux_x86_64.tar.gz";
        let base = serve(HashMap::from([(name.to_owned(), archive.clone())]));
        let check = check_for(
            InstallKind::Standalone,
            &dir,
            AssetKey::LinuxStandalone,
            asset_for(&base, name, &archive),
        );
        let work = root.path().join("work");
        let prepared = prepare(&check, &work, &|_, _| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(prepared.version, "999.0.0");
        assert!(!prepared.runs_installer());
        // nothing changed yet; the archive is gone, the files wait in .update/staged
        assert_eq!(read(&dir.join(APP_BIN)), "old app");
        assert!(!work.join(name).exists());
        let staged = dir.join(UPDATE_DIR).join(STAGED);
        assert_eq!(read(&staged.join(APP_BIN)), "new app");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(staged.join(APP_BIN))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "the program stays executable");
        }
        prepared.finish(false).unwrap();
        assert_updated(&dir);
        assert!(!staged.exists());
        let previous = dir.join(UPDATE_DIR).join(PREVIOUS);
        assert_eq!(read(&previous.join(APP_BIN)), "old app");
        // the next start removes the previous version and the work folder
        fs::create_dir_all(&work).unwrap();
        cleanup(&Installation::new(InstallKind::Standalone, &dir), &work);
        assert!(!dir.join(UPDATE_DIR).exists() && !work.exists());
        assert_updated(&dir);
    }

    #[test]
    fn the_windows_zip_is_unpacked_the_same_way() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("a.zip");
        fs::write(
            &archive,
            zip_file("dentalcraft_999.0.0_windows_x86_64", &NEW_FILES),
        )
        .unwrap();
        let into = root.path().join("staged");
        fs::create_dir_all(&into).unwrap();
        extract_zip(&archive, &into).unwrap();
        assert_eq!(read(&into.join(APP_BIN)), "new app");
        assert_eq!(read(&into.join("LICENSES/OFL.txt")), "new licence");
    }

    const APP: &str = "DentalCraft.app";
    const NEW_BUNDLE: [(&str, &str); 4] = [
        ("Contents/MacOS/dentalcraft", "new app"),
        ("Contents/MacOS/dentalcraft-cli", "new cli"),
        ("Contents/Resources/install-kind", "macos-app\n"),
        ("Contents/Info.plist", "new plist"),
    ];

    /// The `.zip` that `ditto -c -k --keepParent` makes, with the
    /// `__MACOSX` folder and `._` files that extended attributes may add.
    fn app_zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        let plain = zip::write::SimpleFileOptions::default();
        let exec = plain.unix_permissions(0o755);
        zip.add_directory(format!("{APP}/"), plain).unwrap();
        zip.add_directory(format!("{APP}/Contents/"), plain)
            .unwrap();
        for (name, body) in files {
            let opts = if name.contains("/MacOS/") {
                exec
            } else {
                plain
            };
            zip.start_file(format!("{APP}/{name}"), opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.start_file("__MACOSX/._DentalCraft.app", plain).unwrap();
        zip.write_all(b"apple double").unwrap();
        zip.start_file(format!("{APP}/Contents/._Info.plist"), plain)
            .unwrap();
        zip.write_all(b"apple double").unwrap();
        zip.finish().unwrap().into_inner()
    }

    /// An installed bundle `root/Applications/DentalCraft.app` with a file
    /// of the user's next to it.
    fn installed_bundle(root: &Path) -> PathBuf {
        let apps = root.join("Applications");
        let app = apps.join(APP);
        write(&app.join("Contents/MacOS/dentalcraft"), "old app");
        write(&app.join("Contents/MacOS/dentalcraft-cli"), "old cli");
        write(&app.join("Contents/Resources/install-kind"), "macos-app\n");
        write(&app.join("Contents/Info.plist"), "old plist");
        write(&apps.join("Other.app/Contents/Info.plist"), "other");
        app
    }

    fn assert_bundle_updated(app: &Path) {
        assert_eq!(read(&app.join("Contents/MacOS/dentalcraft")), "new app");
        assert_eq!(read(&app.join("Contents/MacOS/dentalcraft-cli")), "new cli");
        assert_eq!(read(&app.join("Contents/Info.plist")), "new plist");
        assert!(!app.join("Contents/._Info.plist").exists());
        assert_eq!(
            read(&app.parent().unwrap().join("Other.app/Contents/Info.plist")),
            "other"
        );
    }

    #[test]
    fn a_macos_bundle_is_unpacked_aside_then_swapped_in_place() {
        let root = tempfile::tempdir().unwrap();
        let app = installed_bundle(root.path());
        let archive = app_zip(&NEW_BUNDLE);
        let name = "dentalcraft_999.0.0_macos_universal.zip";
        let base = serve(HashMap::from([(name.to_owned(), archive.clone())]));
        let check = check_for(
            InstallKind::MacApp,
            &app.join("Contents/MacOS"),
            AssetKey::MacosUniversal,
            asset_for(&base, name, &archive),
        );
        let work = root.path().join("work");
        let prepared = prepare(&check, &work, &|_, _| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(prepared.version, "999.0.0");
        assert!(!prepared.runs_installer());
        // nothing changed yet; the archive is gone; the new bundle waits next to the app
        assert_eq!(read(&app.join("Contents/MacOS/dentalcraft")), "old app");
        assert!(!work.join(name).exists());
        let update = app.with_file_name(".DentalCraft.app.update");
        let staged = update.join(STAGED).join(APP);
        assert_eq!(read(&staged.join("Contents/MacOS/dentalcraft")), "new app");
        assert!(!update.join(STAGED).join("__MACOSX").exists());
        assert!(!staged.join("Contents/._Info.plist").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(staged.join("Contents/MacOS/dentalcraft"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "the program stays executable");
        }
        prepared.finish(false).unwrap();
        assert_bundle_updated(&app);
        assert!(!staged.exists());
        assert_eq!(
            read(
                &update
                    .join(PREVIOUS)
                    .join(APP)
                    .join("Contents/MacOS/dentalcraft")
            ),
            "old app"
        );
        // the next start removes the previous version and the work folder
        fs::create_dir_all(&work).unwrap();
        cleanup(
            &Installation::new(InstallKind::MacApp, app.join("Contents/MacOS")),
            &work,
        );
        assert!(!update.exists() && !work.exists());
        assert_bundle_updated(&app);
    }

    #[test]
    fn a_macos_archive_that_is_not_the_app_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let app = installed_bundle(root.path());
        let name = "dentalcraft_999.0.0_macos_universal.zip";
        let installed = || app.join("Contents/MacOS");
        for archive in [
            // no program in the bundle
            app_zip(&[("Contents/Info.plist", "x")]),
            // a folder that is not an .app
            zip_file("dentalcraft_999.0.0", &NEW_FILES),
        ] {
            let base = serve(HashMap::from([(name.to_owned(), archive.clone())]));
            let check = check_for(
                InstallKind::MacApp,
                &installed(),
                AssetKey::MacosUniversal,
                asset_for(&base, name, &archive),
            );
            let r = prepare(
                &check,
                &root.path().join("work"),
                &|_, _| {},
                &AtomicBool::new(false),
            );
            assert!(matches!(r, Err(UpdateError::Install(_))), "{r:?}");
            assert!(
                !app.with_file_name(".DentalCraft.app.update")
                    .join(STAGED)
                    .exists()
            );
            assert_eq!(read(&app.join("Contents/MacOS/dentalcraft")), "old app");
        }
        // a damaged download changes nothing either
        let archive = app_zip(&NEW_BUNDLE);
        let base = serve(HashMap::from([(name.to_owned(), archive.clone())]));
        let mut asset = asset_for(&base, name, &archive);
        asset.sha256 = crate::download::hex(&Sha256::digest(b"something else"));
        let check = check_for(
            InstallKind::MacApp,
            &installed(),
            AssetKey::MacosUniversal,
            asset,
        );
        let r = prepare(
            &check,
            &root.path().join("work"),
            &|_, _| {},
            &AtomicBool::new(false),
        );
        assert_eq!(r.unwrap_err(), UpdateError::Damaged);
        assert_eq!(read(&app.join("Contents/MacOS/dentalcraft")), "old app");
    }

    #[test]
    fn archive_paths_of_a_bundle_stay_inside_it() {
        let mut top = None;
        let ok = |p: &str, top: &mut Option<OsString>| inside_app(Path::new(p), top);
        assert_eq!(ok("__MACOSX/._DentalCraft.app", &mut top).unwrap(), None);
        assert_eq!(
            ok("DentalCraft.app/", &mut top).unwrap(),
            Some(PathBuf::from(APP))
        );
        assert_eq!(ok("DentalCraft.app/Contents/._x", &mut top).unwrap(), None);
        assert_eq!(
            ok("DentalCraft.app/Contents/Info.plist", &mut top).unwrap(),
            Some(PathBuf::from("DentalCraft.app/Contents/Info.plist"))
        );
        for bad in ["/etc/passwd", "DentalCraft.app/../x", "Other.app/x", "x/y"] {
            assert!(ok(bad, &mut top).is_err(), "{bad}");
        }
        assert!(inside_app(Path::new("notes/x"), &mut None).is_err());
    }

    #[test]
    fn a_failed_bundle_swap_puts_the_installed_one_back() {
        let root = tempfile::tempdir().unwrap();
        let app = installed_bundle(root.path());
        let update = app.with_file_name(".DentalCraft.app.update");
        // the staged bundle is missing: the second rename fails
        let e = swap_bundle(&app, &update.join(STAGED).join(APP), &update);
        assert!(e.is_err());
        assert_eq!(read(&app.join("Contents/MacOS/dentalcraft")), "old app");
        assert_eq!(read(&app.join("Contents/Info.plist")), "old plist");
        // a staged bundle goes in; a second update gets its own previous folder
        for (n, body) in [(1, "second"), (2, "third")] {
            let staged = update.join(STAGED).join(APP);
            write(&staged.join("Contents/Info.plist"), body);
            swap_bundle(&app, &staged, &update).unwrap();
            assert_eq!(read(&app.join("Contents/Info.plist")), body);
            assert_eq!(n == 2, update.join("previous-1").exists());
        }
        // the app was deleted by hand: the new one is still put in place
        fs::remove_dir_all(&app).unwrap();
        let staged = update.join(STAGED).join(APP);
        write(&staged.join("Contents/Info.plist"), "fresh");
        swap_bundle(&app, &staged, &update).unwrap();
        assert_eq!(read(&app.join("Contents/Info.plist")), "fresh");
    }

    #[test]
    fn a_discarded_bundle_update_leaves_the_app_alone() {
        let root = tempfile::tempdir().unwrap();
        let app = installed_bundle(root.path());
        let update = app.with_file_name(".DentalCraft.app.update");
        write(
            &update.join(STAGED).join(APP).join("Contents/Info.plist"),
            "new",
        );
        Prepared::bundle("1.0.0", &app, update.join(STAGED).join(APP), &update).discard();
        assert!(!update.join(STAGED).exists());
        assert_eq!(read(&app.join("Contents/Info.plist")), "old plist");
    }

    #[test]
    fn a_failed_swap_puts_everything_back() {
        let root = tempfile::tempdir().unwrap();
        let dir = installed(root.path());
        let staged = root.path().join("staged");
        for (name, body) in NEW_FILES {
            write(&staged.join(name), body);
        }
        // "README.md" cannot move away: its place in `previous` is taken by a
        // folder that is not empty (entries go in name order: "LICENSES" first)
        let previous = root.path().join("previous");
        write(&previous.join("README.md").join("x"), "busy");
        let e = swap_into(&dir, &staged, &previous);
        assert!(e.is_err());
        assert_eq!(read(&dir.join(APP_BIN)), "old app");
        assert_eq!(read(&dir.join("README.md")), "old readme");
        assert_eq!(read(&dir.join("LICENSES/OFL.txt")), "old licence");
        assert!(!dir.join("install-kind").exists());
        for (name, body) in NEW_FILES {
            assert_eq!(read(&staged.join(name)), body, "{name} is still staged");
        }
        // the same update goes through once the way is clear
        fs::remove_dir_all(previous.join("README.md")).unwrap();
        swap_into(&dir, &staged, &previous).unwrap();
        assert_updated(&dir);
    }

    #[test]
    fn a_previous_version_still_in_use_gets_a_new_folder() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join(UPDATE_DIR);
        let a = new_dir(&parent, PREVIOUS).unwrap();
        let b = new_dir(&parent, PREVIOUS).unwrap();
        assert_ne!(a, b);
        assert!(b.ends_with("previous-1"));
    }

    #[test]
    fn archives_that_reach_outside_are_refused() {
        let mut top = None;
        let ok = |p: &str, top: &mut Option<OsString>| inside_top(Path::new(p), top);
        assert_eq!(ok("dc_1/", &mut top).unwrap(), None);
        assert_eq!(
            ok("./dc_1/bin/x", &mut top).unwrap(),
            Some(PathBuf::from("bin/x"))
        );
        for bad in [
            "/etc/passwd",
            "dc_1/../../x",
            "../x",
            "other_top/x",
            "dc_1/.update/x",
        ] {
            assert!(ok(bad, &mut top).is_err(), "{bad}");
        }
        // a link in a tarball is refused
        let root = tempfile::tempdir().unwrap();
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        tar.append_link(&mut h, "dc_1/evil", "/etc/passwd").unwrap();
        let archive = root.path().join("evil.tar.gz");
        fs::write(&archive, tar.into_inner().unwrap().finish().unwrap()).unwrap();
        let into = root.path().join("into");
        fs::create_dir_all(&into).unwrap();
        assert!(matches!(
            extract_tar_gz(&archive, &into),
            Err(UpdateError::Install(_))
        ));
        assert!(!into.join("evil").exists());
    }

    #[test]
    fn a_damaged_download_changes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let dir = installed(root.path());
        let archive = tar_gz("dentalcraft_999.0.0_linux_x86_64", &NEW_FILES);
        let name = "dentalcraft_999.0.0_linux_x86_64.tar.gz";
        let base = serve(HashMap::from([(name.to_owned(), archive.clone())]));
        let mut asset = asset_for(&base, name, &archive);
        asset.sha256 = crate::download::hex(&Sha256::digest(b"something else"));
        let check = check_for(
            InstallKind::Standalone,
            &dir,
            AssetKey::LinuxStandalone,
            asset,
        );
        let r = prepare(
            &check,
            &root.path().join("work"),
            &|_, _| {},
            &AtomicBool::new(false),
        );
        assert_eq!(r.unwrap_err(), UpdateError::Damaged);
        assert_eq!(read(&dir.join(APP_BIN)), "old app");
        assert!(!dir.join(UPDATE_DIR).join(STAGED).exists());
    }

    #[test]
    fn an_archive_without_the_program_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let dir = installed(root.path());
        let archive = tar_gz("dentalcraft_999.0.0_linux_x86_64", &[("README.md", "x")]);
        let name = "dentalcraft_999.0.0_linux_x86_64.tar.gz";
        let base = serve(HashMap::from([(name.to_owned(), archive.clone())]));
        let check = check_for(
            InstallKind::Standalone,
            &dir,
            AssetKey::LinuxStandalone,
            asset_for(&base, name, &archive),
        );
        let r = prepare(
            &check,
            &root.path().join("work"),
            &|_, _| {},
            &AtomicBool::new(false),
        );
        assert!(matches!(r, Err(UpdateError::Install(_))), "{r:?}");
        assert!(!dir.join(UPDATE_DIR).join(STAGED).exists());
        assert_eq!(read(&dir.join("README.md")), "old readme");
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_folder_is_reported_before_downloading() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let dir = installed(root.path());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        if File::create(dir.join("probe")).is_ok() {
            // running as root: every folder is writable
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        let asset = asset_for("http://127.0.0.1:9", "x.tar.gz", b"x");
        let check = check_for(
            InstallKind::Standalone,
            &dir,
            AssetKey::LinuxStandalone,
            asset,
        );
        let r = prepare(
            &check,
            &root.path().join("work"),
            &|_, _| {},
            &AtomicBool::new(false),
        );
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(r.unwrap_err(), UpdateError::NotWritable(dir));
    }

    #[test]
    fn an_appimage_is_replaced_in_one_rename() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("DentalCraft-0.1.0-x86_64.AppImage");
        fs::write(&target, "old appimage").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let body = b"new appimage".to_vec();
        let name = "DentalCraft-999.0.0-x86_64.AppImage";
        let base = serve(HashMap::from([(name.to_owned(), body.clone())]));
        let mut check = check_for(
            InstallKind::AppImage,
            root.path(),
            AssetKey::AppImage,
            asset_for(&base, name, &body),
        );
        check.installation.appimage = Some(target.clone());
        let prepared = prepare(
            &check,
            &root.path().join("work"),
            &|_, _| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(read(&target), "old appimage");
        let new = appimage_download(&target);
        assert_eq!(read(&new), "new appimage");
        prepared.finish(false).unwrap();
        assert_eq!(read(&target), "new appimage");
        assert!(!new.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&target).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o755,
                "executable for everyone who could run it"
            );
        }
        // a download cut short is removed at the next start
        fs::write(part_path(&new), "half").unwrap();
        let mut inst = Installation::new(InstallKind::AppImage, root.path());
        inst.appimage = Some(target.clone());
        cleanup(&inst, &root.path().join("work"));
        assert!(!part_path(&new).exists());
    }

    #[test]
    fn windows_installers_wait_for_the_app_then_run_quietly() {
        let file = Path::new(
            r"C:\Users\Jane O'Neil\AppData\Roaming\DentalCraft\dentalcraft\data\updates\DentalCraft-1.0.0-setup.exe",
        );
        let dir = Path::new(r"C:\Users\Jane O'Neil\AppData\Local\Programs\DentalCraft");
        let s = installer_script(4242, file, false, dir, true);
        assert!(
            s.starts_with("Wait-Process -Id 4242 -ErrorAction SilentlyContinue; "),
            "{s}"
        );
        assert!(s.contains(r"-FilePath 'C:\Users\Jane O''Neil\AppData\Roaming\DentalCraft\dentalcraft\data\updates\DentalCraft-1.0.0-setup.exe'"), "{s}");
        assert!(
            s.contains(r"@('/S', '/D=C:\Users\Jane O''Neil\AppData\Local\Programs\DentalCraft')"),
            "{s}"
        );
        assert!(s.ends_with(r"{ Start-Process -FilePath 'C:\Users\Jane O''Neil\AppData\Local\Programs\DentalCraft\dentalcraft.exe' }"), "{s}");
        let msi = installer_script(
            7,
            Path::new(r"C:\Temp\Dental Craft.msi"),
            true,
            Path::new(r"C:\Program Files\DentalCraft\"),
            false,
        );
        assert!(
            msi.contains(r#"@('/i', '"C:\Temp\Dental Craft.msi"', '/passive', '/norestart')"#),
            "{msi}"
        );
        assert!(!msi.contains("dentalcraft.exe"), "no restart asked: {msi}");
        // what powershell -EncodedCommand decodes
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded_command(&s))
            .unwrap();
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(String::from_utf16(&units).unwrap(), s);
    }

    #[test]
    fn only_installable_actions_prepare() {
        let root = tempfile::tempdir().unwrap();
        let mut check = check_for(
            InstallKind::Deb,
            root.path(),
            AssetKey::LinuxStandalone,
            asset_for("http://127.0.0.1:9", "x.tar.gz", b"x"),
        );
        check.action = Action::PackageManager { command: "x" };
        let r = prepare(&check, root.path(), &|_, _| {}, &AtomicBool::new(false));
        assert_eq!(r.unwrap_err(), UpdateError::NoDownload);
    }
}
