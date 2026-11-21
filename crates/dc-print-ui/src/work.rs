//! The work of the popup, on a thread of its own so the window never
//! waits: reading the aligners, placing them on plates with their supports
//! and preparing the layers of each plate into memory ([`prepare`]), then
//! writing the files where the user chose ([`save_files`],
//! [`save_plate_models`]). Each run can be cancelled; a file is written to a
//! `.tmp` first and renamed, so a cancelled or failed save leaves no
//! half-written file.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};

use dc_mesh::TriMesh;
use dc_print::job::{self, JobError, PlateSummary, PreparedPlates, PrintJob};
use dc_print::part::PrintPart;

/// A run that failed: the stable code of the error and its detail
/// (untranslated, for the log and the "details" line).
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    /// Stable code (`print-part-too-big`, `print-read`, …).
    pub code: String,
    /// Technical detail.
    pub detail: String,
}

impl Failure {
    pub(crate) fn new(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            detail: detail.into(),
        }
    }

    /// i18n key of the sentence shown for it (`print-err-<code without the
    /// print- prefix>`, `print-err-other` for a code without its own).
    pub fn key(&self) -> String {
        let code = self.code.strip_prefix("print-").unwrap_or(&self.code);
        if KNOWN_CODES.contains(&code) {
            format!("print-err-{code}")
        } else {
            "print-err-other".to_owned()
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.code == "print-cancelled"
    }
}

impl From<JobError> for Failure {
    fn from(e: JobError) -> Self {
        Self::new(e.code(), e.to_string())
    }
}

/// The codes with a sentence of their own (without `print-`).
pub(crate) const KNOWN_CODES: &[&str] = &[
    "no-parts",
    "invalid-part",
    "invalid-settings",
    "fitting-contact",
    "island-unsupported",
    "part-too-tall",
    "part-too-big",
    "invalid-footprint",
    "invalid-layout",
    "file",
    "io",
    "mesh",
    "read",
    "write",
    "internal",
    "no-folder",
];

/// What the window shows of a run under way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// Reading the aligners.
    Reading,
    /// Turning them, adding the supports, placing them on plates.
    Placing,
    /// Preparing the layers of plate `n` (1-based) of `of`.
    Layers { n: u32, of: u32 },
    /// Writing the files.
    Saving,
}

/// Progress of a run, shared with its thread.
#[derive(Debug, Default)]
pub(crate) struct Progress {
    /// Fraction done, 0 to 1, as `f32` bits.
    fraction: AtomicU32,
    /// 0 reading, 1 placing, 2 layers, 3 saving.
    step: AtomicU32,
    plate: AtomicU32,
    plates: AtomicU32,
}

impl Progress {
    fn set(&self, step: Step, fraction: f32) {
        let (code, n, of) = match step {
            Step::Reading => (0, 0, 0),
            Step::Placing => (1, 0, 0),
            Step::Layers { n, of } => (2, n, of),
            Step::Saving => (3, 0, 0),
        };
        self.plate.store(n, Ordering::Relaxed);
        self.plates.store(of, Ordering::Relaxed);
        self.step.store(code, Ordering::Relaxed);
        self.fraction
            .store(fraction.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// The step and the fraction done.
    pub(crate) fn get(&self) -> (Step, f32) {
        let step = match self.step.load(Ordering::Relaxed) {
            0 => Step::Reading,
            1 => Step::Placing,
            2 => Step::Layers {
                n: self.plate.load(Ordering::Relaxed),
                of: self.plates.load(Ordering::Relaxed),
            },
            _ => Step::Saving,
        };
        (step, f32::from_bits(self.fraction.load(Ordering::Relaxed)))
    }
}

/// A run on its own thread.
pub(crate) struct Worker<T> {
    rx: mpsc::Receiver<Result<T, Failure>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<Progress>,
}

impl<T: Send + 'static> Worker<T> {
    /// Runs `f` on a new thread with the cancel flag and the progress.
    pub(crate) fn spawn(
        name: &str,
        f: impl FnOnce(&AtomicBool, &Progress) -> Result<T, Failure> + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Progress::default());
        let (c, p) = (Arc::clone(&cancel), Arc::clone(&progress));
        let spawned = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let _ = tx.send(f(&c, &p));
            });
        if let Err(e) = spawned {
            // the receiver then reports the run as lost
            tracing::error!(code = "print-internal", "cannot start the print work: {e}");
        }
        Self {
            rx,
            cancel,
            progress,
        }
    }
}

impl<T> Worker<T> {
    /// Its answer once it ended; `None` while it runs. A thread that died
    /// (a panic) answers `print-internal`.
    pub(crate) fn try_take(&self) -> Option<Result<T, Failure>> {
        match self.rx.try_recv() {
            Ok(r) => Some(r),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(Failure::new(
                "print-internal",
                "the print work stopped unexpectedly",
            ))),
        }
    }

    /// Asks the run to stop as soon as it can.
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The step and the fraction done.
    pub(crate) fn progress(&self) -> (Step, f32) {
        self.progress.get()
    }
}

impl<T> Drop for Worker<T> {
    fn drop(&mut self) {
        // a run nobody waits for any more stops
        self.cancel();
    }
}

/// An aligner to print: its id and its shell file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Source {
    pub id: String,
    pub path: PathBuf,
}

/// The top view of one part on its plate: the outline of the shell and the
/// outline of its supports, plate millimetres (convex, counter-clockwise).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Outline {
    pub id: String,
    pub part: Vec<[f32; 2]>,
    pub supports: Vec<[f32; 2]>,
}

/// One plate, ready to save.
#[derive(Debug, Clone)]
pub(crate) struct Plate {
    pub summary: PlateSummary,
    /// The print file, as it will be written.
    pub file: Vec<u8>,
    pub outlines: Vec<Outline>,
}

/// The result of [`prepare`].
#[derive(Debug)]
pub(crate) struct Prepared {
    pub plates: Vec<Plate>,
    /// The arranged plates (for the plate models of the advanced view).
    pub arranged: PreparedPlates,
}

impl Prepared {
    /// Ids of every part, plate after plate.
    pub(crate) fn ids(&self) -> Vec<String> {
        self.plates
            .iter()
            .flat_map(|p| p.summary.parts.iter().cloned())
            .collect()
    }
}

/// Reads `sources`, places them with [`job::prepare_cancellable`] and
/// prepares every plate's file in memory with
/// [`job::write_plate_cancellable`].
pub(crate) fn prepare(
    sources: &[Source],
    job: &PrintJob,
    cancel: &AtomicBool,
    progress: &Progress,
) -> Result<Prepared, Failure> {
    let stop = || cancel.load(Ordering::Relaxed);
    let mut parts = Vec::with_capacity(sources.len());
    for (i, s) in sources.iter().enumerate() {
        if stop() {
            return Err(JobError::Cancelled.into());
        }
        progress.set(Step::Reading, 0.05 * i as f32 / sources.len() as f32);
        let mesh = dc_mesh::io::read(&s.path)
            .map_err(|e| Failure::new("print-read", format!("{}: {e}", s.path.display())))?;
        parts.push(PrintPart {
            id: s.id.clone(),
            mesh,
            fitting: None,
        });
    }
    progress.set(Step::Placing, 0.05);
    let arranged = job::prepare_cancellable(&parts, job, &stop)?;
    drop(parts);
    let count = arranged.plates.len();
    let mut plates = Vec::with_capacity(count);
    for (i, plate) in arranged.plates.iter().enumerate() {
        let of = count as u32;
        let n = i as u32 + 1;
        progress.set(Step::Layers { n, of }, 0.1 + 0.9 * i as f32 / count as f32);
        let mut file = Vec::new();
        let summary = job::write_plate_cancellable(
            plate,
            job,
            &mut file,
            &mut |done, total| {
                let f = (i as f32 + done as f32 / total.max(1) as f32) / count as f32;
                progress.set(Step::Layers { n, of }, 0.1 + 0.9 * f);
            },
            &stop,
        )?;
        let outlines = plate
            .parts
            .iter()
            .map(|p| Outline {
                id: p.id.clone(),
                part: hull(std::iter::once(&p.mesh)),
                supports: hull(p.supports.iter()),
            })
            .collect();
        plates.push(Plate {
            summary,
            file,
            outlines,
        });
    }
    progress.set(
        Step::Layers {
            n: count as u32,
            of: count as u32,
        },
        1.0,
    );
    Ok(Prepared { plates, arranged })
}

/// Name of the print file of plate `index` (0-based): `plate_01.goo`, …
pub fn plate_file_name(index: usize) -> String {
    format!("plate_{:02}.goo", index + 1)
}

/// Name of the 3D model of plate `index` (0-based): `plate_01.stl`, …
pub fn plate_model_name(index: usize) -> String {
    format!("plate_{:02}.stl", index + 1)
}

/// Writes each plate's file into `folder` (created when missing); answers
/// the files written.
pub(crate) fn save_files(
    prepared: &Prepared,
    folder: &Path,
    cancel: &AtomicBool,
    progress: &Progress,
) -> Result<Vec<PathBuf>, Failure> {
    let count = prepared.plates.len();
    save_each(
        folder,
        count,
        plate_file_name,
        cancel,
        progress,
        |i, out| out.write_all(&prepared.plates[i].file),
    )
}

/// Writes each arranged plate (shells and supports) as one STL into
/// `folder`; answers the files written.
pub(crate) fn save_plate_models(
    prepared: &Prepared,
    folder: &Path,
    cancel: &AtomicBool,
    progress: &Progress,
) -> Result<Vec<PathBuf>, Failure> {
    let count = prepared.arranged.plates.len();
    save_each(
        folder,
        count,
        plate_model_name,
        cancel,
        progress,
        |i, out| {
            job::write_plate_stl(&prepared.arranged.plates[i], out)
                .map_err(|e| std::io::Error::other(e.to_string()))
        },
    )
}

/// Writes `count` files into `folder`, file `i` named `name(i)`, each
/// through a `.tmp` renamed once complete; `write(i, out)` writes file `i`.
fn save_each(
    folder: &Path,
    count: usize,
    name: fn(usize) -> String,
    cancel: &AtomicBool,
    progress: &Progress,
    mut write: impl FnMut(usize, &mut dyn Write) -> std::io::Result<()>,
) -> Result<Vec<PathBuf>, Failure> {
    let io = |path: &Path, e: std::io::Error| {
        Failure::new("print-write", format!("{}: {e}", path.display()))
    };
    fs::create_dir_all(folder).map_err(|e| io(folder, e))?;
    let mut written = Vec::with_capacity(count);
    for i in 0..count {
        if cancel.load(Ordering::Relaxed) {
            return Err(JobError::Cancelled.into());
        }
        progress.set(Step::Saving, i as f32 / count.max(1) as f32);
        let tmp = folder.join(format!(".dentalcraft-{i}.tmp"));
        let written_ok = {
            let file = fs::File::create(&tmp).map_err(|e| io(&tmp, e))?;
            let mut out = BufWriter::new(file);
            write(i, &mut out).and_then(|()| {
                let file = out.into_inner().map_err(|e| e.into_error())?;
                file.sync_all()
            })
        };
        if let Err(e) = written_ok {
            let _ = fs::remove_file(&tmp);
            return Err(io(&tmp, e));
        }
        let path = folder.join(name(i));
        fs::rename(&tmp, &path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            io(&path, e)
        })?;
        written.push(path);
    }
    progress.set(Step::Saving, 1.0);
    Ok(written)
}

/// Convex hull of the points of `meshes` seen from above (monotone chain),
/// counter-clockwise; empty without points.
fn hull<'a>(meshes: impl Iterator<Item = &'a TriMesh>) -> Vec<[f32; 2]> {
    let mut pts: Vec<[f32; 2]> = meshes
        .flat_map(|m| m.positions.iter().map(|p| [p.x, p.y]))
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .collect();
    convex_hull(&mut pts)
}

/// Convex hull of `pts` (sorted in place), counter-clockwise.
pub(crate) fn convex_hull(pts: &mut [[f32; 2]]) -> Vec<[f32; 2]> {
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(2 * pts.len());
    for pass in 0..2 {
        let start = out.len();
        let iter: Box<dyn Iterator<Item = &[f32; 2]>> = if pass == 0 {
            Box::new(pts.iter())
        } else {
            Box::new(pts.iter().rev())
        };
        for &p in iter {
            while out.len() >= start + 2 && cross(out[out.len() - 2], out[out.len() - 1], p) <= 0.0
            {
                out.pop();
            }
            out.push(p);
        }
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_of_a_square_with_inner_points() {
        let mut pts = vec![
            [0.0, 0.0],
            [1.0, 0.0],
            [0.5, 0.5],
            [1.0, 1.0],
            [0.0, 1.0],
            [0.2, 0.7],
        ];
        let h = convex_hull(&mut pts);
        assert_eq!(h, vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
    }

    #[test]
    fn every_known_code_has_its_key_and_others_fall_back() {
        assert_eq!(
            Failure::new("print-part-too-big", "").key(),
            "print-err-part-too-big"
        );
        assert_eq!(Failure::new("strange", "").key(), "print-err-other");
        let en = include_str!("../i18n/en/dc-print-ui.ftl");
        for code in KNOWN_CODES {
            assert!(en.contains(&format!("print-err-{code} = ")), "{code}");
        }
        assert!(en.contains("print-err-other = "));
    }

    #[test]
    fn files_are_written_whole_and_named_by_plate() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let progress = Progress::default();
        let out = save_each(
            dir.path(),
            2,
            plate_file_name,
            &cancel,
            &progress,
            |i, w| w.write_all(&[i as u8; 3]),
        )
        .unwrap();
        assert_eq!(
            out,
            vec![
                dir.path().join("plate_01.goo"),
                dir.path().join("plate_02.goo")
            ]
        );
        assert_eq!(fs::read(&out[1]).unwrap(), vec![1, 1, 1]);
        let left: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(left.len(), 2, "no .tmp is left");
    }

    #[test]
    fn a_cancelled_save_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(true);
        let r = save_each(
            dir.path(),
            1,
            plate_file_name,
            &cancel,
            &Progress::default(),
            |_, _| Ok(()),
        );
        assert!(r.unwrap_err().is_cancelled());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
