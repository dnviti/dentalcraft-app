//! Installing a newer version: [`prepare`] downloads and checks it and puts
//! it in place next to the installation, [`Prepared::finish`] switches to it
//! (when the program closes) and can start it.
//!
//! | installation | prepare | finish |
//! |---|---|---|
//! | standalone (`.tar.gz`, `.zip`) | unpacks the archive into `<folder>/.update/staged/` | moves each unpacked entry into the folder, the one it replaces into `<folder>/.update/previous/` (renames only, undone on failure); a running program may be renamed on Linux and Windows |
//! | AppImage | downloads next to `$APPIMAGE` (`.<name>.new`) | renames it over `$APPIMAGE` (atomic) |
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
        let update = installation.dir.join(UPDATE_DIR);
        for entry in fs::read_dir(&update).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().starts_with(PREVIOUS) {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
        // only when nothing else is in it
        let _ = fs::remove_dir(&update);
    }
    if let Some(target) = &installation.appimage {
        let new = appimage_download(target);
        let _ = fs::remove_file(part_path(&new));
        let _ = fs::remove_file(new);
    }
    let _ = fs::remove_dir_all(work_dir);
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
        let Some(rel) = inside_top(&name, &mut top)? else {
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
