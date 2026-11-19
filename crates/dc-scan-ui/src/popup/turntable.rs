//! The turntable of the scan popup (alpha): its block above the steps
//! (picker, connection, position, manual moves, calibration) and the
//! automatic scan of the Acquisition step.
//!
//! The popup knows the turntable only through the `turntable.*` commands
//! and their JSON (the same an agent reads), so any turntable a backend of
//! `dc-turntable` brings shows here without a change.

use std::time::{SystemTime, UNIX_EPOCH};

use dc_scanner::ScanState;
use dc_scanner::control::{
    SCAN_AUTO, TURNTABLE_CALIBRATE, TURNTABLE_CONNECT, TURNTABLE_DEVICES, TURNTABLE_DISCONNECT,
    TURNTABLE_HOME, TURNTABLE_MOVE, TURNTABLE_SPEED, TURNTABLE_STOP,
};
use dc_scanner::rig::{
    AUTO_FRAMES_PER_STOP, AUTO_RESOLUTION_MM, AUTO_STEP_DEG, AUTO_TILTS_DEG, ROI_BELOW_PLATTER_MM,
};
use dc_ui::fonts;
use dc_ui::icons as ic;
use dc_ui::theme::colors;
use dc_ui::widgets::{self, BadgeKind, Button};
use egui::{Align, Layout, RichText, Sense, Stroke, Ui, vec2};
use glam::Vec3;
use serde_json::{Value, json};

use super::{Act, ScanUi, Snapshot};
use crate::{ScanFailure, caption, t, tr_n};

/// Seconds the Refresh button looks for turntables (Bluetooth ones answer
/// within a few seconds of their advertising).
pub(super) const SEARCH_S: f32 = 5.0;
/// Seconds the popup looks for turntables when it opens.
pub(crate) const OPEN_SEARCH_S: f32 = 1.0;

/// Tilts of the presets of the automatic scan: Arch (the tilts `scan.auto`
/// scans at by default), Quick.
const PRESETS: [&[f32]; 2] = [&AUTO_TILTS_DEG, &[0.0]];
/// Tilt range when the turntable does not say its own.
const TILT_RANGE: (f32, f32) = (-30.0, 30.0);

/// A turntable as `turntable.devices` lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct TableInfo {
    pub backend: String,
    pub id: String,
    pub name: String,
}

impl TableInfo {
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            backend: v["backend"].as_str()?.to_owned(),
            id: v["id"].as_str()?.to_owned(),
            name: v["name"].as_str().unwrap_or_default().to_owned(),
        })
    }
}

/// The turntable as `turntable.status` describes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct TableSnap {
    pub connected: bool,
    pub info: Option<TableInfo>,
    /// The tilt axis and its range, when the turntable tilts.
    pub tilt: Option<(f32, f32)>,
    pub rotate_deg: Option<f32>,
    pub tilt_deg: Option<f32>,
    /// Moving.
    pub busy: bool,
}

fn num(v: &Value) -> Option<f32> {
    v.as_f64().map(|x| x as f32)
}

impl TableSnap {
    pub fn from_json(v: &Value) -> Self {
        let tilt = v["axes"].as_array().and_then(|axes| {
            axes.iter().find(|a| a["kind"] == "tilt").map(|a| {
                (
                    num(&a["min_deg"]).unwrap_or(TILT_RANGE.0),
                    num(&a["max_deg"]).unwrap_or(TILT_RANGE.1),
                )
            })
        });
        Self {
            connected: v["connected"].as_bool().unwrap_or(false),
            info: TableInfo::from_json(&v["info"]),
            tilt,
            rotate_deg: num(&v["rotate_deg"]),
            tilt_deg: num(&v["tilt_deg"]),
            busy: v["busy"].as_bool().unwrap_or(false),
        }
    }

    fn tilt_range(&self) -> (f32, f32) {
        self.tilt.unwrap_or(TILT_RANGE)
    }
}

/// A background job of the rig (`RigJobStatus`: the automatic scan, the
/// calibration).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct JobSnap {
    /// The pass under way, from 1.
    pub step: u32,
    pub steps: u32,
    pub tilt_deg: Option<f32>,
    /// Running or paused (else it ended).
    pub active: bool,
    /// Paused (`scan.pause`): the turntable waits.
    pub paused: bool,
    /// The code of the error of a failed job.
    pub error: Option<ScanFailure>,
    /// `turntable.calibrate`: what the run measured.
    pub calibration: Option<CalReport>,
    /// `scan.auto`: how the views were registered and fused.
    pub quality: Option<QualityReport>,
    /// `scan.auto`: the phase of the job (`acquire`, then the phases of the
    /// finished mesh); `None` for a job that does not name one.
    pub phase: Option<String>,
    /// `scan.auto`: the fine reconstruction from the stops.
    pub reconstruction: Option<ReconSnap>,
    /// `scan.auto`: what the mesh finish changed.
    pub finish: Option<FinishSnap>,
    /// Times the link to the turntable dropped and came back during the job.
    pub reconnects: u32,
    /// The link dropped and the job is reconnecting the turntable now.
    pub reconnecting: bool,
    /// `scan.auto` stop-and-capture, once ended: the stops kept for a
    /// rebuild (also when the job failed or was cancelled).
    pub kept_stops: Option<u32>,
}

impl JobSnap {
    pub fn from_json(v: &Value) -> Option<Self> {
        let steps = u32::try_from(v["steps"].as_u64()?).ok()?;
        let step = u32::try_from(v["step"].as_u64().unwrap_or(1)).ok()?;
        let state = v["state"].as_str().unwrap_or_default();
        Some(Self {
            step: step.clamp(1, steps.max(1)),
            steps,
            tilt_deg: num(&v["tilt_deg"]),
            active: matches!(state, "running" | "paused"),
            paused: state == "paused",
            error: (state == "failed").then(|| {
                ScanFailure::new(
                    v["error"].as_str().unwrap_or("other"),
                    v["error_detail"].as_str().unwrap_or_default(),
                )
            }),
            calibration: v.get("calibration").and_then(CalReport::from_json),
            quality: v.get("quality").and_then(QualityReport::from_json),
            phase: v["phase"].as_str().map(str::to_owned),
            reconstruction: v.get("reconstruction").and_then(ReconSnap::from_json),
            finish: v.get("finish").and_then(FinishSnap::from_json),
            reconnects: u32::try_from(count(v, "reconnects")).unwrap_or(u32::MAX),
            reconnecting: v["reconnecting"].as_bool().unwrap_or(false),
            kept_stops: v["kept_stops"].as_u64().and_then(|n| u32::try_from(n).ok()),
        })
    }

    /// The job is past the acquisition, building the surface: the scanner
    /// and the turntable are idle and only the phase shows.
    pub fn finishing(&self) -> bool {
        self.active && self.phase.as_deref().is_some_and(|p| p != "acquire")
    }

    /// The job while it runs.
    pub fn running(this: Option<&Self>) -> Option<&Self> {
        this.filter(|j| j.active)
    }

    /// How far it is, `0..=1`.
    fn fraction(&self) -> f32 {
        self.step.saturating_sub(1) as f32 / self.steps.max(1) as f32
    }
}

/// What a calibration run measured (`RigJobStatus::calibration`).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CalReport {
    pub rms_deg: Option<f32>,
    pub rms_mm: Option<f32>,
    /// Stop pairs registered, and tried.
    pub pairs: u32,
    pub pairs_expected: u32,
    /// Measured over commanded turn.
    pub angle_gain: Option<f32>,
}

impl CalReport {
    fn from_json(v: &Value) -> Option<Self> {
        v.is_object().then(|| Self {
            rms_deg: num(&v["rms_deg"]),
            rms_mm: num(&v["rms_mm"]),
            pairs: v["pairs"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            pairs_expected: v["pairs_expected"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            angle_gain: num(&v["angle_gain"]),
        })
    }
}

/// How well an automatic scan registered and fused its views
/// (`RigJobStatus::quality`).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct QualityReport {
    /// Stops captured (0 when turning continuously).
    pub stops: u32,
    /// Stops registered from the turntable pose only.
    pub flagged_stops: u32,
    /// Frames received while turning (continuous).
    pub frames: u64,
    /// Frames or stops fused.
    pub fused: u64,
    pub mean_rms_mm: Option<f32>,
    /// The loop closure error of each full turn (degrees).
    pub loop_error_deg: Vec<f32>,
    /// The code of the warning of a weak scan.
    pub warning: Option<String>,
}

impl QualityReport {
    fn from_json(v: &Value) -> Option<Self> {
        if !v.is_object() {
            return None;
        }
        let count = |k: &str| v[k].as_u64().unwrap_or(0);
        Some(Self {
            stops: u32::try_from(count("stops")).unwrap_or(u32::MAX),
            flagged_stops: u32::try_from(count("flagged_stops")).unwrap_or(u32::MAX),
            frames: count("frames"),
            fused: count("fused"),
            mean_rms_mm: num(&v["mean_rms_mm"]),
            loop_error_deg: v["loop_error_deg"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(num)
                .collect(),
            warning: v["warning"].as_str().map(str::to_owned),
        })
    }

    /// The largest loop closure error (degrees), when a full turn was made.
    fn loop_error(&self) -> Option<f32> {
        self.loop_error_deg
            .iter()
            .map(|e| e.abs())
            .max_by(f32::total_cmp)
    }
}

/// The sentence of the warning code of a weak scan.
fn warning_key(code: &str) -> &'static str {
    match code {
        "scan-quality-weak" => "scan-warn-scan-quality-weak",
        _ => "scan-warn-other",
    }
}

/// The sentence key of the phase of an automatic scan that builds the mesh;
/// `None` while it acquires (or the job names no phase).
fn phase_key(phase: &str) -> Option<&'static str> {
    Some(match phase {
        "acquire" => return None,
        "refine" => "scan-phase-refine",
        "fuse" => "scan-phase-fuse",
        "extract" => "scan-phase-extract",
        "clean" => "scan-phase-clean",
        "fill-holes" => "scan-phase-fill-holes",
        "optimise" => "scan-phase-optimise",
        _ => "scan-phase-other",
    })
}

fn count(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or(0)
}

/// The fine reconstruction from the stops (`scan.auto`'s `reconstruction`,
/// the answer of `scan.reconstruct`).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ReconSnap {
    pub resolution_mm: Option<f32>,
    pub stops: u64,
    /// Stops whose position was refined before fusing (0: not refined).
    pub refined: u64,
    /// Mean distance between views before the refinement.
    pub mean_rms_before_mm: Option<f32>,
    pub mean_rms_mm: Option<f32>,
    pub vertices: u64,
    pub faces: u64,
    pub seconds: Option<f32>,
    /// Stops left out because they disagree with the others (the piece
    /// may have moved), whole tilt passes left out, and stops registered
    /// again on their own.
    pub rejected_stops: u64,
    pub rejected_passes: u64,
    pub rescued_stops: u64,
    /// Per tilt pass, by tilt.
    pub passes: Vec<PassSnap>,
    /// The code of the warning of a reconstruction that left views out.
    pub warning: Option<String>,
}

impl ReconSnap {
    pub fn from_json(v: &Value) -> Option<Self> {
        v.is_object().then(|| Self {
            resolution_mm: num(&v["resolution_mm"]),
            stops: count(v, "stops"),
            // a count of stops, or only whether the positions were refined
            refined: v["refined"].as_u64().unwrap_or_else(|| {
                if v["refined"].as_bool().unwrap_or(false) {
                    count(v, "stops")
                } else {
                    0
                }
            }),
            mean_rms_before_mm: num(&v["mean_rms_before_mm"]),
            mean_rms_mm: num(&v["mean_rms_mm"]),
            vertices: count(v, "vertices"),
            faces: count(v, "faces"),
            seconds: num(&v["seconds"]),
            rejected_stops: count(v, "rejected_stops"),
            rejected_passes: count(v, "rejected_passes"),
            rescued_stops: count(v, "rescued_stops"),
            passes: v["pass_details"]
                .as_array()
                .into_iter()
                .flatten()
                .map(PassSnap::from_json)
                .collect(),
            warning: v["warning"].as_str().map(str::to_owned),
        })
    }
}

/// One tilt pass of a reconstruction.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PassSnap {
    pub tilt_deg: f32,
    pub stops: u64,
    /// Rotation (degrees) and shift (mm) of the correction of the pass.
    pub corrected_deg: f32,
    pub corrected_mm: f32,
    pub rescued_stops: u64,
    pub rejected_stops: u64,
    /// The whole pass was left out.
    pub rejected: bool,
}

impl PassSnap {
    fn from_json(v: &Value) -> Self {
        Self {
            tilt_deg: num(&v["tilt_deg"]).unwrap_or(0.0),
            stops: count(v, "stops"),
            corrected_deg: num(&v["corrected_deg"]).unwrap_or(0.0),
            corrected_mm: num(&v["corrected_mm"]).unwrap_or(0.0),
            rescued_stops: count(v, "rescued_stops"),
            rejected_stops: count(v, "rejected_stops"),
            rejected: v["rejected"].as_bool().unwrap_or(false),
        }
    }
}

/// The tilts as the custom field writes them (`-20, 0, 20`).
fn tilts_text(tilts: &[f32]) -> String {
    tilts
        .iter()
        .map(f32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the mesh finish did (`scan.auto`'s `finish`, the answer of the
/// `finish` op of `scan.mesh`).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FinishSnap {
    pub islands_removed: u64,
    pub nonmanifold_fixed: u64,
    pub holes_filled: u64,
    pub holes_left: u64,
    pub faces_before: u64,
    pub faces_after: u64,
    pub max_error_mm: Option<f32>,
}

impl FinishSnap {
    pub fn from_json(v: &Value) -> Option<Self> {
        v.is_object().then(|| Self {
            islands_removed: count(v, "islands_removed"),
            nonmanifold_fixed: count(v, "nonmanifold_fixed"),
            holes_filled: count(v, "holes_filled"),
            holes_left: count(v, "holes_left"),
            faces_before: count(v, "faces_before"),
            faces_after: count(v, "faces_after"),
            max_error_mm: num(&v["max_error_mm"]),
        })
    }
}

/// The stored calibration of the rig (`turntable.calibration`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct CalSnap {
    pub turntable_id: Option<String>,
    pub scanner_serial: Option<String>,
    /// When it was made, in seconds since 1970.
    pub created: Option<u64>,
    /// Errors of the fit: the turn in degrees, the axis position in mm
    /// (both 0 when the calibration did not store them).
    pub fit_rms_deg: f32,
    pub fit_rms_mm: f32,
}

impl CalSnap {
    pub fn from_json(v: &Value) -> Option<Self> {
        if !v.is_object() {
            return None;
        }
        Some(Self {
            turntable_id: v["turntable_id"].as_str().map(str::to_owned),
            scanner_serial: v["scanner_serial"].as_str().map(str::to_owned),
            created: match &v["created"] {
                Value::Number(n) => n.as_u64(),
                Value::String(s) => rfc3339_secs(s),
                _ => None,
            },
            fit_rms_deg: num(&v["fit_rms_deg"]).unwrap_or(0.0),
            fit_rms_mm: num(&v["fit_rms_mm"]).unwrap_or(0.0),
        })
    }

    /// Made with this turntable and this scanner (as
    /// `RigCalibration::matches`: a serial missing on either side does not
    /// count against it).
    fn matches(&self, scanner_serial: Option<&str>, turntable_id: Option<&str>) -> bool {
        self.turntable_id.as_deref() == turntable_id
            && match (self.scanner_serial.as_deref(), scanner_serial) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }
}

/// How good the axis fit of a calibration is, from the errors it stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CalQuality {
    Good,
    Fair,
    Poor,
}

impl CalQuality {
    /// Errors (turn in degrees, axis position in mm) up to which the fit
    /// is good, then fair. The calibration itself refuses a fit beyond
    /// 2.5° or 2 mm per stop pair, so beyond FAIR is an older calibration.
    const GOOD: (f32, f32) = (1.0, 1.0);
    const FAIR: (f32, f32) = (2.5, 2.0);

    fn of(deg: f32, mm: f32) -> Self {
        let within = |(d, m): (f32, f32)| deg <= d && mm <= m;
        if within(Self::GOOD) {
            Self::Good
        } else if within(Self::FAIR) {
            Self::Fair
        } else {
            Self::Poor
        }
    }

    fn badge(self) -> BadgeKind {
        match self {
            Self::Good => BadgeKind::Success,
            Self::Fair => BadgeKind::Warning,
            Self::Poor => BadgeKind::Danger,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Good => "turntable-cal-quality-good",
            Self::Fair => "turntable-cal-quality-fair",
            Self::Poor => "turntable-cal-quality-poor",
        }
    }
}

impl CalSnap {
    /// The quality of the fit and its errors (turn in degrees, axis
    /// position in mm); `None` for a calibration that did not store them.
    fn fit(&self) -> Option<(CalQuality, f32, f32)> {
        let (deg, mm) = (self.fit_rms_deg, self.fit_rms_mm);
        (deg != 0.0 || mm != 0.0).then(|| (CalQuality::of(deg, mm), deg, mm))
    }
}

/// Box around the cylinder of the region of interest (`radius_mm` around
/// the axis, from under the platter to `height_mm` above it) in the scan
/// world of a calibrated rig (+Z the axis, origin on the platter).
pub(super) fn roi_box(radius_mm: f32, height_mm: f32) -> (Vec3, Vec3) {
    (
        Vec3::new(-radius_mm, -radius_mm, -ROI_BELOW_PLATTER_MM),
        Vec3::new(radius_mm, radius_mm, height_mm),
    )
}

/// The turntable rotation as an orientation (degrees in (−180, 180]): the
/// table counts whole turns (a full turn reads 360), the platter does not.
fn wrapped(deg: f32) -> f32 {
    let w = deg.rem_euclid(360.0);
    if w > 180.0 { w - 360.0 } else { w }
}

/// The turning speed the popup starts with (°/s): slow enough for the
/// scanner to follow at its 3 to 10 frames per second.
const DEFAULT_SPEED_DEG_S: f32 = 8.0;
/// Degrees the turntable may turn between two frames before tracking
/// is at risk.
const MAX_DEG_PER_FRAME: f32 = 2.5;
/// Stop and capture, the first of the two choices (the other is continuous
/// turning), in [`TableUi::capture`].
const CAPTURE_STOP: usize = 0;
/// Resolutions of the mesh the automatic scan and the manual rebuild offer
/// (mm), all inside the range of `scan.reconstruct`.
pub(super) const MESH_RESOLUTIONS_MM: [f32; 5] = [0.08, 0.1, 0.15, 0.2, 0.3];

/// The choice of [`MESH_RESOLUTIONS_MM`] nearest to the resolution
/// `scan.auto` defaults to.
fn default_resolution() -> usize {
    MESH_RESOLUTIONS_MM
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            (*a - AUTO_RESOLUTION_MM)
                .abs()
                .total_cmp(&(*b - AUTO_RESOLUTION_MM).abs())
        })
        .map_or(0, |(i, _)| i)
}

/// Degrees turned between two frames and the speed (°/s) that keeps it near
/// 2°, when `speed` (°/s) turns the object too far between two frames at
/// `fps` frames per second; `None` when it is slow enough or the frame rate
/// is not known.
fn too_fast(speed: f32, fps: f32) -> Option<(f32, f32)> {
    (fps >= 0.5 && speed / fps > MAX_DEG_PER_FRAME).then(|| (speed / fps, (fps * 2.0).max(2.0)))
}

/// The advice for [`too_fast`].
fn speed_hint(speed: f32, fps: f32) -> Option<String> {
    let (per_frame, suggested) = too_fast(speed, fps)?;
    Some(dc_i18n::tr_args(
        "turntable-speed-hint",
        &[
            ("fps", &format!("{fps:.1}")),
            ("deg", &format!("{per_frame:.1}")),
            ("speed", &format!("{suggested:.0}")),
        ],
    ))
}

/// The calibration of the rig made with the connected scanner and
/// turntable, if any.
pub(super) fn matching(snap: &Snapshot) -> Option<&CalSnap> {
    let serial = snap
        .status
        .device
        .as_ref()
        .and_then(|d| d.serial.as_deref());
    let table = snap.table.info.as_ref().map(|i| i.id.as_str());
    snap.calibration
        .as_ref()
        .filter(|c| c.matches(serial, table))
}

/// Seconds since 1970 of an RFC 3339 time (`2026-11-16T14:09:00Z`, an
/// offset or fractions allowed).
fn rfc3339_secs(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (h, mi, sec) = (n(11..13)?, n(14..16)?, n(17..19)?);
    // the offset, after the fractions of a second
    let rest = s[19..].trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest.as_bytes().first() {
        None | Some(b'Z' | b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let oh = rest.get(1..3)?.parse::<i64>().ok()?;
            let om = rest.get(4..6)?.parse::<i64>().ok()?;
            let o = oh * 3600 + om * 60;
            if *sign == b'+' { o } else { -o }
        }
        _ => return None,
    };
    // days from the civil date (Howard Hinnant's algorithm)
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + mi * 60 + sec - offset).ok()
}

/// The sentence of a calibration made `secs` seconds ago.
fn calibrated_ago(secs: u64) -> String {
    match secs {
        0..60 => t("turntable-cal-now"),
        60..3600 => tr_n("turntable-cal-minutes", (secs / 60).to_string()),
        3600..172_800 => tr_n("turntable-cal-hours", (secs / 3600).to_string()),
        _ => tr_n("turntable-cal-days", (secs / 86_400).to_string()),
    }
}

/// The tilts written by hand (`-20, 0, +20`), each within `lo..=hi`;
/// `None` when one is not a number, is out of range, or none is written.
pub(super) fn parse_tilts(text: &str, lo: f32, hi: f32) -> Option<Vec<f32>> {
    let tilts: Option<Vec<f32>> = text
        .split([',', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let s = s.replace('−', "-").replace('°', "");
            let s = s.trim().trim_start_matches('+');
            s.parse::<f32>()
                .ok()
                .filter(|v| v.is_finite() && (lo..=hi).contains(v))
        })
        .collect();
    tilts.filter(|v| !v.is_empty())
}

/// A user-facing name of a turntable backend (`revopoint-ble`, `sim`).
fn backend_name(id: &str) -> String {
    let key = format!("turntable-backend-{id}");
    let name = t(&key);
    if name == key { id.to_owned() } else { name }
}

/// A signed angle as the popup shows it (`+12.5`, `−3.0`).
pub(super) fn deg(v: f32) -> String {
    let v = if v.abs() < 0.05 { 0.0 } else { v };
    if v < 0.0 {
        format!("−{:.1}", -v)
    } else {
        format!("{v:.1}")
    }
}

/// What the turntable block edits.
pub(super) struct TableUi {
    pub devices: Vec<TableInfo>,
    /// Backends that cannot be used here, with the code of the reason.
    pub unavailable: Vec<(String, Option<String>)>,
    pub device: usize,
    pub step_deg: f32,
    pub tilt_deg: f32,
    pub speed: f32,
    /// Auto-scan preset: Arch, Quick, Custom.
    pub preset: usize,
    pub custom: String,
    pub rotation_deg: f32,
    /// The manual controls and the calibration are shown.
    pub expanded: bool,
    /// The automatic scan asked for without a calibration of this rig: its
    /// parameters, while the confirmation is open.
    pub confirm_auto: Option<Value>,
    /// "Calibrate first" was chosen: the calibration control comes into
    /// view with the focus on the next frame.
    pub focus_calibrate: bool,
    /// Stop and capture ([`CAPTURE_STOP`]) or continuous turning.
    pub capture: usize,
    /// Degrees between the stops of stop and capture.
    pub stop_step_deg: f32,
    /// Frames taken at each stop (a whole number).
    pub frames_per_stop: f32,
    /// Resolution of the finished mesh: a choice of [`MESH_RESOLUTIONS_MM`].
    pub resolution: usize,
    /// The mesh is cleaned, its holes closed and optimised when the scan
    /// ends.
    pub finish: bool,
    /// The positions of the stops are refined before the views are fused.
    pub refine: bool,
}

impl Default for TableUi {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
            unavailable: Vec::new(),
            device: 0,
            step_deg: 45.0,
            tilt_deg: 0.0,
            speed: DEFAULT_SPEED_DEG_S,
            preset: 0,
            custom: tilts_text(&AUTO_TILTS_DEG),
            rotation_deg: 360.0,
            expanded: true,
            confirm_auto: None,
            focus_calibrate: false,
            capture: CAPTURE_STOP,
            stop_step_deg: AUTO_STEP_DEG,
            frames_per_stop: AUTO_FRAMES_PER_STOP as f32,
            resolution: default_resolution(),
            finish: true,
            refine: true,
        }
    }
}

impl TableUi {
    /// The answer of `turntable.devices`.
    pub fn listed(&mut self, out: &Value) {
        self.devices = out["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(TableInfo::from_json)
            .collect();
        self.unavailable = out["backends"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["available"] == json!(false))
            .map(|b| {
                (
                    b["id"].as_str().unwrap_or_default().to_owned(),
                    b["error_code"].as_str().map(str::to_owned),
                )
            })
            .collect();
        // a real turntable first, the simulated one after
        if let Some(i) = self.devices.iter().position(|d| d.backend != "sim") {
            self.device = i;
        }
        self.device = self.device.min(self.devices.len().saturating_sub(1));
    }

    /// The answer of `turntable.connect` (`{turntable, axes, calibrated}`):
    /// the turntable an agent connected may not be listed yet.
    pub fn connected(&mut self, out: &Value) {
        let Some(d) = TableInfo::from_json(&out["turntable"]) else {
            return;
        };
        self.device = match self.devices.iter().position(|x| x.id == d.id) {
            Some(i) => i,
            None => {
                self.devices.push(d);
                self.devices.len() - 1
            }
        };
    }

    /// The tilts of the automatic scan, `None` while the custom list is
    /// not valid.
    fn tilts(&self, range: (f32, f32)) -> Option<Vec<f32>> {
        match PRESETS.get(self.preset) {
            Some(p) => Some(p.to_vec()),
            None => parse_tilts(&self.custom, range.0, range.1),
        }
    }

    /// The parameters of `scan.auto`: with a calibration of the rig stop and
    /// capture (the turntable moves between the stops at its own pace) unless
    /// continuous turning is chosen; without one, continuous turning.
    fn auto_params(&self, tilts: Vec<f32>, calibrated: bool) -> Value {
        let mut params = json!({
            "tilts_deg": tilts,
            "rotation_deg": self.rotation_deg,
            "resolution_mm": self.resolution_mm(),
            "finish": self.finish,
            "refine": self.refine,
        });
        if calibrated && self.capture == CAPTURE_STOP {
            params["mode"] = json!("stop-and-capture");
            params["step_deg"] = json!(self.stop_step_deg);
            params["frames_per_stop"] = json!(self.frames_per_stop.round() as u32);
        } else {
            params["mode"] = json!("continuous");
            params["deg_per_s"] = json!(self.speed);
        }
        params
    }

    /// The resolution of the mesh chosen (mm).
    pub fn resolution_mm(&self) -> f32 {
        MESH_RESOLUTIONS_MM
            .get(self.resolution)
            .copied()
            .unwrap_or(AUTO_RESOLUTION_MM)
    }

    /// Stop and capture or continuous turning, the angle between the stops
    /// and the frames taken at each. Without a calibration only continuous
    /// turning works: the control says so.
    fn capture_ui(&mut self, ui: &mut Ui, calibrated: bool) {
        ui.add_space(6.0);
        if !calibrated {
            caption(ui, t("scan-auto-uncalibrated-hint"));
            return;
        }
        let labels = [t("scan-auto-mode-stop"), t("scan-auto-mode-continuous")];
        widgets::segmented_fill(ui, &mut self.capture, &labels, 32.0);
        if self.capture == CAPTURE_STOP {
            ui.add_space(6.0);
            widgets::labeled_slider(
                ui,
                &t("scan-auto-stop-step"),
                &mut self.stop_step_deg,
                2.0..=45.0,
                0.5,
                1,
                "°",
                AUTO_STEP_DEG,
            );
            ui.add_space(6.0);
            widgets::labeled_slider(
                ui,
                &t("scan-auto-stop-frames"),
                &mut self.frames_per_stop,
                1.0..=10.0,
                1.0,
                0,
                "",
                AUTO_FRAMES_PER_STOP as f32,
            );
            ui.add_space(4.0);
            caption(ui, t("scan-auto-stop-hint"));
        }
    }

    /// The quality of the mesh: its resolution, the refinement of the
    /// positions and, for the automatic scan (`with_finish`), the finish
    /// at the end. Shared by the automatic scan and the manual rebuild.
    pub fn quality_ui(&mut self, ui: &mut Ui, with_finish: bool) {
        caption(ui, t("scan-quality-resolution"));
        ui.add_space(2.0);
        let labels: Vec<String> = MESH_RESOLUTIONS_MM
            .iter()
            .map(|mm| format!("{mm:.2}"))
            .collect();
        widgets::segmented_fill(ui, &mut self.resolution, &labels, 30.0);
        ui.add_space(2.0);
        caption(
            ui,
            tr_n(
                "scan-quality-resolution-mm",
                format!("{:.2}", self.resolution_mm()),
            ),
        );
        ui.add_space(6.0);
        widgets::toggle(ui, &mut self.refine, t("scan-quality-refine"));
        ui.add_space(2.0);
        caption(ui, t("scan-quality-refine-hint"));
        if with_finish {
            ui.add_space(6.0);
            widgets::toggle(ui, &mut self.finish, t("scan-quality-finish"));
            ui.add_space(2.0);
            caption(ui, t("scan-quality-finish-hint"));
        }
    }
}

/// The heading of the turntable block: its icon in a circle (it is not a
/// step), its name, the alpha badge and the button that folds its
/// controls.
fn heading(ui: &mut Ui, expanded: &mut bool) {
    let c = *colors(ui);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
        ui.painter()
            .circle_stroke(rect.center(), 10.0, Stroke::new(1.5, c.select_blue));
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            ic::DISC,
            fonts::icon(12.0),
            c.select_blue,
        );
        ui.label(
            RichText::new(t("turntable-title"))
                .font(fonts::semibold(14.0))
                .color(c.text_primary),
        );
        widgets::badge_icon(ui, BadgeKind::Amber, None, t("scan-alpha"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (icon, key) = if *expanded {
                (ic::CARET_UP, "turntable-fold")
            } else {
                (ic::CARET_DOWN, "turntable-unfold")
            };
            if widgets::icon_button(ui, icon, t(key)).clicked() {
                *expanded = !*expanded;
            }
        });
    });
    ui.add_space(8.0);
}

/// A small heading inside a block.
pub(super) fn sub_heading(ui: &mut Ui, key: &str) {
    let c = *colors(ui);
    ui.label(
        RichText::new(t(key))
            .font(fonts::semibold(13.0))
            .color(c.text_primary),
    );
    ui.add_space(4.0);
}

/// `true` when a slider was let go or typed into (a change to apply).
fn released(r: &egui::Response) -> bool {
    r.drag_stopped() || (r.changed() && !r.dragged())
}

/// A job under way: a spinner (a pause sign while paused) with `text`,
/// and its progress bar.
fn progress(ui: &mut Ui, text: &str, job: &JobSnap) {
    let c = *colors(ui);
    ui.horizontal(|ui| {
        if job.paused {
            ui.label(
                RichText::new(ic::PAUSE)
                    .font(fonts::icon(14.0))
                    .color(c.status_warning),
            );
        } else {
            widgets::spinner(ui, 14.0);
        }
        ui.label(
            RichText::new(text)
                .font(fonts::medium(13.0))
                .color(c.text_primary),
        );
    });
    ui.add_space(4.0);
    let width = ui.available_width();
    widgets::progress_bar(ui, Some(job.fraction()), width);
}

/// The warning that the link to the turntable dropped `count` times and
/// the job went on.
fn reconnect_warning(ui: &mut Ui, count: u32) {
    let c = *colors(ui);
    ui.add_space(4.0);
    ui.label(
        RichText::new(format!(
            "{} {}",
            ic::WARNING,
            dc_i18n::tr_args("scan-auto-reconnected", &[("count", &count.to_string())])
        ))
        .font(fonts::ui(12.0))
        .color(c.status_warning),
    );
}

/// The progress of a calibration under way: its move, or the reconnection
/// of the turntable when the link dropped.
fn calibration_progress(ui: &mut Ui, job: &JobSnap) {
    let text = if job.reconnecting {
        t("turntable-reconnecting")
    } else {
        tr_n(
            "turntable-cal-progress",
            format!("{} / {}", job.step, job.steps),
        )
    };
    progress(ui, &text, job);
    if job.reconnects > 0 {
        reconnect_warning(ui, job.reconnects);
    }
}

/// What a calibration run measured: the errors of the fit, the stop pairs
/// registered and the angle gain.
fn calibration_report(ui: &mut Ui, r: &CalReport) {
    if let (Some(deg), Some(mm)) = (r.rms_deg, r.rms_mm) {
        caption(
            ui,
            dc_i18n::tr_args(
                "turntable-cal-residuals",
                &[("deg", &format!("{deg:.2}")), ("mm", &format!("{mm:.2}"))],
            ),
        );
    }
    if r.pairs_expected > 0 {
        caption(
            ui,
            dc_i18n::tr_args(
                "turntable-cal-pairs",
                &[
                    ("pairs", &r.pairs.to_string()),
                    ("expected", &r.pairs_expected.to_string()),
                ],
            ),
        );
    }
    if let Some(gain) = r.angle_gain {
        caption(
            ui,
            dc_i18n::tr_args("turntable-cal-gain", &[("gain", &format!("{gain:.3}"))]),
        );
    }
}

/// What the fine reconstruction measured and built.
pub(super) fn recon_report(ui: &mut Ui, r: &ReconSnap) {
    caption(
        ui,
        dc_i18n::tr_args(
            "scan-recon-stops",
            &[
                ("stops", &r.stops.to_string()),
                (
                    "mm",
                    &r.resolution_mm
                        .map_or_else(|| "-".to_owned(), |mm| format!("{mm:.2}")),
                ),
            ],
        ),
    );
    let mm = |v: f32| format!("{v:.2}");
    let refined = r.refined.to_string();
    let stops = r.stops.to_string();
    match (r.refined, r.mean_rms_mm, r.mean_rms_before_mm) {
        (0, Some(now), _) => caption(
            ui,
            dc_i18n::tr_args("scan-quality-residual", &[("mm", &mm(now))]),
        ),
        (0, None, _) => {}
        (_, Some(now), Some(before)) => caption(
            ui,
            dc_i18n::tr_args(
                "scan-recon-refined-from",
                &[
                    ("refined", &refined),
                    ("stops", &stops),
                    ("before", &mm(before)),
                    ("mm", &mm(now)),
                ],
            ),
        ),
        (_, Some(now), None) => caption(
            ui,
            dc_i18n::tr_args(
                "scan-recon-refined",
                &[("refined", &refined), ("stops", &stops), ("mm", &mm(now))],
            ),
        ),
        (_, None, _) => caption(
            ui,
            dc_i18n::tr_args(
                "scan-recon-refined-plain",
                &[("refined", &refined), ("stops", &stops)],
            ),
        ),
    }
    caption(
        ui,
        dc_i18n::tr_args(
            "scan-recon-mesh",
            &[
                ("vertices", &r.vertices.to_string()),
                ("faces", &r.faces.to_string()),
                (
                    "seconds",
                    &r.seconds
                        .map_or_else(|| "-".to_owned(), |s| format!("{s:.1}")),
                ),
            ],
        ),
    );
    // what was left out, and what was registered again
    if r.rejected_stops > 0 || r.rejected_passes > 0 {
        caption(
            ui,
            dc_i18n::tr_args(
                "scan-recon-left-out",
                &[
                    ("stops", &r.rejected_stops.to_string()),
                    ("passes", &r.rejected_passes.to_string()),
                ],
            ),
        );
    }
    if r.rescued_stops > 0 {
        caption(
            ui,
            dc_i18n::tr_args(
                "scan-recon-rescued",
                &[("count", &r.rescued_stops.to_string())],
            ),
        );
    }
    for p in &r.passes {
        let tilt = deg(p.tilt_deg);
        if p.rejected {
            caption(
                ui,
                dc_i18n::tr_args("scan-recon-pass-rejected", &[("tilt", &tilt)]),
            );
        } else {
            caption(
                ui,
                dc_i18n::tr_args(
                    "scan-recon-pass",
                    &[
                        ("tilt", &tilt),
                        ("stops", &p.stops.to_string()),
                        ("deg", &format!("{:.2}", p.corrected_deg)),
                        ("mm", &format!("{:.2}", p.corrected_mm)),
                        ("rejected", &p.rejected_stops.to_string()),
                    ],
                ),
            );
        }
    }
    if r.warning.is_some() {
        let c = *colors(ui);
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!(
                "{} {}",
                ic::WARNING,
                dc_i18n::tr_args(
                    "scan-warn-views-rejected",
                    &[("count", &r.rejected_stops.to_string())],
                )
            ))
            .font(fonts::ui(12.0))
            .color(c.status_warning),
        );
    }
}

/// What the mesh finish removed, closed and optimised.
pub(super) fn finish_report(ui: &mut Ui, f: &FinishSnap) {
    caption(
        ui,
        dc_i18n::tr_args(
            "scan-finish-cleaned",
            &[
                ("islands", &f.islands_removed.to_string()),
                ("fixed", &f.nonmanifold_fixed.to_string()),
            ],
        ),
    );
    caption(
        ui,
        dc_i18n::tr_args(
            "scan-finish-holes",
            &[
                ("filled", &f.holes_filled.to_string()),
                ("left", &f.holes_left.to_string()),
            ],
        ),
    );
    let (before, after) = (f.faces_before.to_string(), f.faces_after.to_string());
    match f.max_error_mm {
        Some(mm) => caption(
            ui,
            dc_i18n::tr_args(
                "scan-finish-faces",
                &[
                    ("before", &before),
                    ("after", &after),
                    ("mm", &format!("{mm:.3}")),
                ],
            ),
        ),
        None => caption(
            ui,
            dc_i18n::tr_args(
                "scan-finish-faces-plain",
                &[("before", &before), ("after", &after)],
            ),
        ),
    }
}

/// How the latest automatic scan went: the views fused and flagged, the
/// residual, the loop closure error, the fine reconstruction, what the
/// finish did and the warning of a weak scan.
fn auto_report(ui: &mut Ui, job: &JobSnap) {
    let c = *colors(ui);
    if job.quality.is_none()
        && job.reconstruction.is_none()
        && job.finish.is_none()
        && job.reconnects == 0
        && job.kept_stops.is_none()
    {
        return;
    }
    ui.add_space(8.0);
    sub_heading(ui, "scan-quality-title");
    if let Some(q) = &job.quality {
        let line = if q.stops > 0 {
            dc_i18n::tr_args(
                "scan-quality-stops",
                &[
                    ("fused", &q.fused.to_string()),
                    ("stops", &q.stops.to_string()),
                    ("flagged", &q.flagged_stops.to_string()),
                ],
            )
        } else {
            dc_i18n::tr_args(
                "scan-quality-frames",
                &[
                    ("fused", &q.fused.to_string()),
                    ("frames", &q.frames.to_string()),
                ],
            )
        };
        caption(ui, line);
        // the reconstruction reports its own residual, after refining
        if job.reconstruction.is_none()
            && let Some(mm) = q.mean_rms_mm
        {
            caption(
                ui,
                dc_i18n::tr_args("scan-quality-residual", &[("mm", &format!("{mm:.2}"))]),
            );
        }
        if let Some(deg) = q.loop_error() {
            caption(
                ui,
                dc_i18n::tr_args("scan-quality-loop", &[("deg", &format!("{deg:.2}"))]),
            );
        }
    }
    if let Some(r) = &job.reconstruction {
        recon_report(ui, r);
    }
    if let Some(f) = &job.finish {
        finish_report(ui, f);
    }
    if let Some(code) = job.quality.as_ref().and_then(|q| q.warning.as_ref()) {
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{} {}", ic::WARNING, t(warning_key(code))))
                .font(fonts::ui(12.0))
                .color(c.status_warning),
        );
    }
    if job.reconnects > 0 {
        reconnect_warning(ui, job.reconnects);
    }
    // a scan without a finished surface keeps its stops for a rebuild
    if let Some(n) = job.kept_stops.filter(|&n| n > 0)
        && job.finish.is_none()
    {
        ui.add_space(4.0);
        caption(
            ui,
            dc_i18n::tr_args("scan-kept-stops", &[("count", &n.to_string())]),
        );
        caption(ui, t("scan-kept-stops-hint"));
    }
}

impl ScanUi {
    /// The turntable block, above the steps.
    pub(super) fn turntable_ui(
        &mut self,
        ui: &mut Ui,
        snap: &Snapshot,
        busy: bool,
        out: &mut Vec<Act>,
    ) {
        let c = *colors(ui);
        heading(ui, &mut self.table.expanded);
        let table = &snap.table;
        let job = JobSnap::running(snap.auto.as_ref())
            .or(JobSnap::running(snap.calibrating.as_ref()))
            .is_some();
        // the link dropped: the table is known but not reachable
        let dropped = !table.connected && table.info.is_some();
        let reconnecting = snap.reconnecting();
        // the surface is being built: nothing moves any more
        let finishing = snap.finishing();

        // picker, refresh, connection
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_enabled_ui(!table.connected && !dropped && !busy, |ui| {
                let shown = table
                    .info
                    .as_ref()
                    .map(|i| i.name.clone())
                    .unwrap_or_else(|| {
                        self.table
                            .devices
                            .get(self.table.device)
                            .map_or_else(|| t("turntable-no-device"), |d| d.name.clone())
                    });
                egui::ComboBox::from_id_salt("dc-scan-turntable")
                    .width(ui.available_width() - 40.0)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for (i, d) in self.table.devices.iter().enumerate() {
                            ui.selectable_value(&mut self.table.device, i, &d.name);
                        }
                    });
            });
            if widgets::icon_button(ui, ic::ARROWS_CLOCKWISE, t("turntable-refresh")).clicked()
                && !busy
                && !table.connected
            {
                out.push(Act::Run(
                    TURNTABLE_DEVICES,
                    json!({ "timeout_s": SEARCH_S }),
                ));
            }
        });
        // the kinds of turntable that cannot be used here, and why
        for (backend, code) in &self.table.unavailable {
            let why = code.as_deref().map_or_else(
                || t("scan-err-other"),
                |code| t(&ScanFailure::new(code, "").key()),
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(
                    RichText::new(ic::INFO)
                        .font(fonts::icon(12.0))
                        .color(c.text_secondary),
                );
                caption(ui, format!("{}: {why}", backend_name(backend)));
            });
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if table.connected {
                if Button::secondary(t("turntable-disconnect"))
                    .icon(ic::PLUG)
                    .enabled(!busy && !job)
                    .show(ui)
                    .clicked()
                {
                    out.push(Act::Run(TURNTABLE_DISCONNECT, Value::Null));
                }
            } else {
                // a dropped link connects the same table again
                let params = match (&table.info, dropped) {
                    (Some(i), true) => json!({ "backend": i.backend, "device": i.id }),
                    _ => self.table.devices.get(self.table.device).map_or(
                        Value::Null,
                        |d| json!({ "backend": d.backend, "device": d.id }),
                    ),
                };
                if Button::secondary(t("turntable-connect"))
                    .icon(ic::PLUGS_CONNECTED)
                    .enabled(!busy && !reconnecting && (dropped || !self.table.devices.is_empty()))
                    .show(ui)
                    .clicked()
                {
                    out.push(Act::Run(TURNTABLE_CONNECT, params));
                }
            }
            // a paused automatic scan holds the turntable still
            let paused = snap.auto.as_ref().is_some_and(|j| j.paused);
            let (kind, key) = match (table.connected, table.busy && !paused && !finishing) {
                _ if reconnecting => (BadgeKind::Warning, "turntable-state-reconnecting"),
                (false, _) if dropped => (BadgeKind::Danger, "turntable-state-dropped"),
                (false, _) => (BadgeKind::Neutral, "turntable-state-off"),
                (true, false) => (BadgeKind::Info, "turntable-state-ready"),
                (true, true) => (BadgeKind::Warning, "turntable-state-moving"),
            };
            widgets::badge(ui, kind, t(key));
        });
        if !table.connected {
            // a job that reconnects the table shows it, and its progress
            if reconnecting {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("{} {}", ic::WARNING, t("turntable-reconnecting")))
                        .font(fonts::ui(12.0))
                        .color(c.status_warning),
                );
            }
            if let Some(j) = JobSnap::running(snap.calibrating.as_ref()) {
                ui.add_space(6.0);
                calibration_progress(ui, j);
            }
            return;
        }

        // where it is
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let text = |ui: &mut Ui, s: String| {
                ui.label(
                    RichText::new(s)
                        .font(fonts::ui(13.0))
                        .color(c.text_secondary),
                );
            };
            if let Some(r) = table.rotate_deg {
                text(ui, tr_n("turntable-rotation", deg(wrapped(r))));
            }
            if table.tilt.is_some()
                && let Some(v) = table.tilt_deg
            {
                text(ui, tr_n("turntable-tilt-now", deg(v)));
            }
        });
        // while the automatic scan runs the manual controls (unusable then)
        // fold, so its progress in Acquisition stays in view
        if !self.table.expanded || JobSnap::running(snap.auto.as_ref()).is_some() {
            if let Some(j) = JobSnap::running(snap.calibrating.as_ref()) {
                ui.add_space(6.0);
                calibration_progress(ui, j);
            }
            return;
        }

        // moves by hand
        let manual = !busy && !job;
        ui.add_space(8.0);
        ui.add_enabled_ui(manual, |ui| {
            widgets::labeled_slider(
                ui,
                &t("turntable-step"),
                &mut self.table.step_deg,
                1.0..=360.0,
                1.0,
                0,
                "°",
                45.0,
            );
            ui.horizontal(|ui| {
                let half = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
                for (key, icon, sign) in [
                    ("turntable-turn-back", ic::ARROW_COUNTER_CLOCKWISE, -1.0),
                    ("turntable-turn-on", ic::ARROW_CLOCKWISE, 1.0),
                ] {
                    if Button::secondary(t(key))
                        .icon(icon)
                        .enabled(manual)
                        .min_width(half)
                        .show(ui)
                        .clicked()
                    {
                        out.push(Act::Run(
                            TURNTABLE_MOVE,
                            json!({ "axis": "rotate", "by_deg": sign * self.table.step_deg }),
                        ));
                    }
                }
            });
            if let Some((lo, hi)) = table.tilt {
                ui.add_space(6.0);
                self.table.tilt_deg = self.table.tilt_deg.clamp(lo, hi);
                widgets::labeled_slider_deg(
                    ui,
                    &t("turntable-tilt"),
                    &mut self.table.tilt_deg,
                    lo..=hi,
                    0.0,
                );
                if Button::secondary(t("turntable-tilt-apply"))
                    .icon(ic::ANGLE)
                    .enabled(manual)
                    .full_width(true)
                    .show(ui)
                    .clicked()
                {
                    out.push(Act::Run(
                        TURNTABLE_MOVE,
                        json!({ "axis": "tilt", "to_deg": self.table.tilt_deg }),
                    ));
                }
            }
            ui.add_space(6.0);
            let r = widgets::labeled_slider(
                ui,
                &t("turntable-speed"),
                &mut self.table.speed,
                2.0..=60.0,
                1.0,
                0,
                &t("turntable-speed-unit"),
                DEFAULT_SPEED_DEG_S,
            );
            if released(&r) {
                out.push(Act::Run(
                    TURNTABLE_SPEED,
                    json!({ "axis": "rotate", "deg_per_s": self.table.speed }),
                ));
            }
            if let Some(hint) = speed_hint(self.table.speed, snap.status.fps) {
                caption(ui, hint);
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            // Stop works whenever the turntable is connected and no other
            // command holds the scanner (a running command has its own Stop)
            if Button::secondary(t("turntable-stop"))
                .icon(ic::STOP)
                .enabled(!busy)
                .show(ui)
                .clicked()
            {
                out.push(Act::Run(TURNTABLE_STOP, Value::Null));
            }
            if Button::ghost(t("turntable-home"))
                .icon(ic::HOUSE_SIMPLE)
                .enabled(manual)
                .show(ui)
                .clicked()
            {
                out.push(Act::Run(TURNTABLE_HOME, Value::Null));
            }
        });

        // calibration
        ui.add_space(12.0);
        sub_heading(ui, "turntable-cal-title");
        caption(ui, t("turntable-cal-hint"));
        ui.add_space(6.0);
        // what the latest calibration run measured, also when it failed
        let report = snap
            .calibrating
            .as_ref()
            .filter(|j| !j.active)
            .and_then(|j| j.calibration.as_ref());
        match &snap.calibration {
            Some(cal) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let other = matching(snap).is_none();
                if other {
                    ui.label(
                        RichText::new(format!("{} {}", ic::WARNING, t("turntable-cal-other")))
                            .font(fonts::ui(12.0))
                            .color(c.status_warning),
                    );
                } else {
                    let text = cal.created.map_or_else(
                        || t("turntable-cal-now"),
                        |at| calibrated_ago(now.saturating_sub(at)),
                    );
                    let fit = cal.fit();
                    ui.horizontal_wrapped(|ui| {
                        widgets::badge(ui, BadgeKind::Success, text);
                        if let Some((quality, _, _)) = fit {
                            widgets::badge(ui, quality.badge(), t(quality.key()));
                        }
                    });
                    if let Some((quality, deg, mm)) = fit {
                        if report.is_none() {
                            ui.add_space(4.0);
                            caption(
                                ui,
                                dc_i18n::tr_args(
                                    "turntable-cal-residuals",
                                    &[("deg", &format!("{deg:.2}")), ("mm", &format!("{mm:.2}"))],
                                ),
                            );
                        }
                        if quality == CalQuality::Poor {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(format!(
                                    "{} {}",
                                    ic::WARNING,
                                    t("turntable-cal-poor-hint")
                                ))
                                .font(fonts::ui(12.0))
                                .color(c.status_warning),
                            );
                        }
                    }
                }
            }
            None => {
                widgets::badge(ui, BadgeKind::Neutral, t("turntable-cal-none"));
            }
        }
        if let Some(r) = report {
            ui.add_space(4.0);
            calibration_report(ui, r);
        }
        ui.add_space(6.0);
        if let Some(j) = JobSnap::running(snap.calibrating.as_ref()) {
            calibration_progress(ui, j);
            return;
        }
        let scanner =
            snap.status.state == ScanState::Connected || snap.status.state == ScanState::Stopped;
        if snap.status.state == ScanState::Idle {
            caption(ui, t("turntable-cal-needs"));
            ui.add_space(4.0);
        }
        let key = if snap.calibration.is_some() {
            "turntable-recalibrate"
        } else {
            "turntable-calibrate"
        };
        let r = Button::secondary(t(key))
            .icon(ic::CROSSHAIR)
            .enabled(scanner && manual)
            .full_width(true)
            .show(ui);
        if std::mem::take(&mut self.table.focus_calibrate) {
            r.scroll_to_me(Some(Align::Center));
            r.request_focus();
        }
        if r.clicked() {
            out.push(Act::Run(TURNTABLE_CALIBRATE, Value::Null));
        }
    }

    /// The automatic scan, in the Acquisition step.
    pub(super) fn auto_ui(
        &mut self,
        ui: &mut Ui,
        snap: &Snapshot,
        busy: bool,
        primary: bool,
        out: &mut Vec<Act>,
    ) {
        ui.add_space(10.0);
        sub_heading(ui, "scan-auto-title");
        let table = &snap.table;
        if let Some(a) = JobSnap::running(snap.auto.as_ref()) {
            // once the turning is done the job builds the surface (the
            // acquisition is stopped, maybe before the phase says so)
            let processing = snap.finishing();
            let phase = a
                .phase
                .as_deref()
                .and_then(phase_key)
                .or(processing.then_some("scan-phase-other"));
            let text = match phase {
                Some(key) => dc_i18n::tr_args(
                    "scan-auto-phase",
                    &[
                        ("step", &a.step.to_string()),
                        ("steps", &a.steps.to_string()),
                        ("what", &t(key)),
                    ],
                ),
                None => dc_i18n::tr_args(
                    "scan-auto-progress",
                    &[
                        ("step", &a.step.to_string()),
                        ("steps", &a.steps.to_string()),
                        ("tilt", &deg(a.tilt_deg.unwrap_or(0.0))),
                    ],
                ),
            };
            let text = if a.reconnecting {
                t("turntable-reconnecting")
            } else {
                text
            };
            progress(ui, &text, a);
            ui.add_space(4.0);
            let hint = if phase.is_some() {
                "scan-auto-phase-hint"
            } else {
                "scan-auto-hint"
            };
            caption(ui, t(hint));
            // Stop cancels the job; the stops taken so far are kept
            if processing {
                ui.add_space(6.0);
                if Button::secondary(t("scan-stop"))
                    .icon(ic::STOP)
                    .full_width(true)
                    .show(ui)
                    .clicked()
                {
                    out.push(Act::Run(TURNTABLE_STOP, Value::Null));
                }
            }
            if a.reconnects > 0 {
                reconnect_warning(ui, a.reconnects);
            }
            return;
        }
        if !table.connected {
            caption(ui, t("scan-auto-needs"));
            // what the scan that ended left, also when the link dropped
            if !busy && let Some(j) = snap.auto.as_ref() {
                auto_report(ui, j);
            }
            return;
        }
        let busy = busy || JobSnap::running(snap.calibrating.as_ref()).is_some();
        let range = table.tilt_range();
        let can = matches!(snap.status.state, ScanState::Connected | ScanState::Stopped) && !busy;
        let tilts = ui
            .add_enabled_ui(can, |ui| {
                if table.tilt.is_some() {
                    let labels = [
                        tr_n("scan-auto-arch", AUTO_TILTS_DEG.len().to_string()),
                        t("scan-auto-quick"),
                        t("scan-auto-custom"),
                    ];
                    widgets::segmented_fill(ui, &mut self.table.preset, &labels, 32.0);
                    ui.add_space(6.0);
                } else {
                    // a turntable that only turns: one pass, flat
                    self.table.preset = 1;
                }
                let tilts = self.table.tilts(range);
                if self.table.preset >= PRESETS.len() {
                    let error = tilts.is_none().then(|| {
                        dc_i18n::tr_args(
                            "scan-auto-tilts-error",
                            &[("min", &deg(range.0)), ("max", &deg(range.1))],
                        )
                    });
                    widgets::text_input(
                        ui,
                        &t("scan-auto-tilts"),
                        &mut self.table.custom,
                        &tilts_text(&AUTO_TILTS_DEG),
                        None,
                        error.as_deref(),
                    );
                    ui.add_space(6.0);
                }
                widgets::labeled_slider(
                    ui,
                    &t("scan-auto-rotation"),
                    &mut self.table.rotation_deg,
                    30.0..=720.0,
                    15.0,
                    0,
                    "°",
                    360.0,
                );
                self.table.capture_ui(ui, matching(snap).is_some());
                ui.add_space(10.0);
                sub_heading(ui, "scan-quality-settings");
                // only the stop and capture scan keeps the stops the fine
                // mesh is built from
                let stops = matching(snap).is_some() && self.table.capture == CAPTURE_STOP;
                ui.add_enabled_ui(stops, |ui| self.table.quality_ui(ui, true));
                if !stops {
                    ui.add_space(4.0);
                    caption(ui, t("scan-quality-needs-stops"));
                }
                tilts
            })
            .inner;
        ui.add_space(6.0);
        let b = if primary {
            Button::primary(t("scan-auto-start"))
        } else {
            Button::secondary(t("scan-auto-start"))
        };
        if b.icon(ic::PLAY_CIRCLE)
            .enabled(can && tilts.is_some())
            .full_width(true)
            .show(ui)
            .clicked()
            && let Some(tilts) = tilts
        {
            let params = self.table.auto_params(tilts, matching(snap).is_some());
            if matching(snap).is_some() {
                out.push(Act::Run(SCAN_AUTO, params));
            } else {
                // without a calibration of this rig the background turns
                // with nothing: ask first
                self.table.confirm_auto = Some(params);
            }
        }
        ui.add_space(4.0);
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            caption(ui, t("scan-auto-hint"));
        });
        if !busy && let Some(j) = snap.auto.as_ref() {
            auto_report(ui, j);
        }
    }

    /// The confirmation of an automatic scan without a calibration of this
    /// rig: calibrate first (the calibration control comes into view) or
    /// scan anyway (`allow_uncalibrated`).
    pub(super) fn auto_confirm(&mut self, ui: &Ui, out: &mut Vec<Act>) {
        let Some(params) = self.table.confirm_auto.clone() else {
            return;
        };
        let c = *colors(ui);
        let resp = egui::Modal::new(egui::Id::unique("dc-scan-auto-confirm"))
            .frame(widgets::card_frame(ui).inner_margin(egui::Margin::same(24)))
            .show(ui.ctx(), |ui| {
                ui.set_width(440.0);
                ui.label(
                    RichText::new(t("scan-auto-confirm-title"))
                        .font(fonts::semibold(18.0))
                        .color(c.text_primary),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(t("scan-auto-confirm-body"))
                        .font(fonts::ui(13.0))
                        .color(c.text_secondary),
                );
                ui.add_space(16.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Button::primary(t("scan-auto-confirm-calibrate"))
                        .icon(ic::CROSSHAIR)
                        .show(ui)
                        .clicked()
                    {
                        return Some(false);
                    }
                    if Button::secondary(t("scan-auto-confirm-anyway"))
                        .show(ui)
                        .clicked()
                    {
                        return Some(true);
                    }
                    None
                })
                .inner
            });
        match resp.inner {
            Some(true) => {
                let mut params = params;
                params["allow_uncalibrated"] = json!(true);
                out.push(Act::Run(SCAN_AUTO, params));
                self.table.confirm_auto = None;
            }
            Some(false) => {
                self.table.confirm_auto = None;
                self.table.expanded = true;
                self.table.focus_calibrate = true;
            }
            None if resp.should_close() => self.table.confirm_auto = None,
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilts_written_by_hand() {
        assert_eq!(
            parse_tilts("-20, 0, +20", -30.0, 30.0),
            Some(vec![-20.0, 0.0, 20.0])
        );
        assert_eq!(
            parse_tilts("−15°; 15", -30.0, 30.0),
            Some(vec![-15.0, 15.0])
        );
        assert_eq!(parse_tilts("0,", -30.0, 30.0), Some(vec![0.0]));
        assert_eq!(parse_tilts("", -30.0, 30.0), None);
        assert_eq!(parse_tilts("10, x", -30.0, 30.0), None);
        assert_eq!(parse_tilts("40", -30.0, 30.0), None);
        assert_eq!(parse_tilts("NaN", -30.0, 30.0), None);
    }

    #[test]
    fn calibration_times() {
        assert_eq!(rfc3339_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_secs("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(
            rfc3339_secs("2026-11-16T14:09:00.123+01:00"),
            rfc3339_secs("2026-11-16T13:09:00Z")
        );
        assert_eq!(rfc3339_secs("yesterday"), None);
        let cal = CalSnap::from_json(&json!({ "turntable_id": "t", "created": 5 })).unwrap();
        assert_eq!(cal.created, Some(5));
        assert_eq!(CalSnap::from_json(&Value::Null), None);
    }

    #[test]
    fn status_of_the_turntable() {
        let s = TableSnap::from_json(&json!({
            "connected": true,
            "info": { "backend": "sim", "id": "sim-turntable", "name": "Sim" },
            "axes": [
                { "kind": "rotate", "relative_only": true, "feedback": "polled" },
                { "kind": "tilt", "min_deg": -30.0, "max_deg": 30.0, "relative_only": false, "feedback": "none" }
            ],
            "rotate_deg": -134.9,
            "busy": true,
        }));
        assert!(s.connected && s.busy);
        assert_eq!(s.tilt, Some((-30.0, 30.0)));
        assert_eq!(s.info.unwrap().id, "sim-turntable");
        assert_eq!(s.tilt_deg, None);
        assert_eq!(TableSnap::from_json(&Value::Null), TableSnap::default());
        let a = JobSnap::from_json(
            &json!({ "kind": "auto", "state": "running", "step": 2, "steps": 3, "tilt_deg": 0.0 }),
        )
        .unwrap();
        assert_eq!((a.step, a.steps, a.active, a.paused), (2, 3, true, false));
        let p = JobSnap::from_json(&json!({ "state": "paused", "step": 1, "steps": 2 })).unwrap();
        assert!(p.active && p.paused);
        assert!((a.fraction() - 1.0 / 3.0).abs() < 1e-6);
        let f = JobSnap::from_json(&json!({
            "kind": "calibrate", "state": "failed", "step": 1, "steps": 9,
            "error": "turntable-protocol", "error_detail": "bad reply",
        }))
        .unwrap();
        assert!(JobSnap::running(Some(&f)).is_none());
        assert_eq!(f.error.unwrap().code, "turntable-protocol");
        assert_eq!(deg(-0.01), "0.0");
        assert_eq!(deg(-3.0), "−3.0");
    }

    #[test]
    fn unavailable_backends_keep_their_code() {
        let mut ui = TableUi::default();
        ui.listed(&json!({
            "devices": [
                { "backend": "sim", "id": "sim-turntable", "name": "Simulated turntable" }
            ],
            "backends": [
                { "id": "revopoint-ble", "available": false, "error_code": "ble-no-adapter" },
                { "id": "sim", "available": true }
            ],
        }));
        assert_eq!(ui.devices.len(), 1);
        assert_eq!(
            ui.unavailable,
            vec![(
                "revopoint-ble".to_owned(),
                Some("ble-no-adapter".to_owned())
            )]
        );
        assert_eq!(
            t(&ScanFailure::new("ble-no-adapter", "").key()),
            t("scan-err-ble-no-adapter")
        );
        assert!(backend_name("revopoint-ble").contains("Bluetooth"));
        assert_eq!(backend_name("other"), "other");
    }

    fn calibration(extra: Value) -> CalSnap {
        let mut v = json!({
            "turntable_id": "t",
            "axis_point": [0.0, 0.0, 200.0],
            "axis_dir": [0.0, -1.0, 0.0],
            "platter_up": [0.0, -1.0, 0.0],
        });
        for (k, x) in extra.as_object().into_iter().flatten() {
            v[k] = x.clone();
        }
        CalSnap::from_json(&v).unwrap()
    }

    #[test]
    fn quality_of_the_fit() {
        let q = |d, m| calibration(json!({ "fit_rms_deg": d, "fit_rms_mm": m })).fit();
        assert_eq!(q(0.1, 0.05).unwrap().0, CalQuality::Good);
        assert_eq!(q(0.9, 0.9).unwrap().0, CalQuality::Good);
        assert_eq!(q(1.7, 1.3).unwrap().0, CalQuality::Fair);
        assert_eq!(q(0.2, 1.5).unwrap().0, CalQuality::Fair);
        assert_eq!(q(0.2, 2.5).unwrap().0, CalQuality::Poor);
        let (_, deg, mm) = q(0.12, 0.34).unwrap();
        assert!((deg - 0.12).abs() < 1e-6 && (mm - 0.34).abs() < 1e-6);
        // a calibration made before the errors were stored says nothing
        assert_eq!(calibration(json!({})).fit(), None);
        assert_eq!(q(0.0, 0.0), None);
        assert_eq!(CalQuality::of(f32::NAN, 0.1), CalQuality::Poor);
        assert_eq!(CalQuality::Good.badge(), BadgeKind::Success);
        assert_eq!(CalQuality::Poor.badge(), BadgeKind::Danger);
    }

    #[test]
    fn box_of_the_region_of_interest() {
        // the scan world of a calibrated rig: +Z the axis, origin on the
        // platter, so the cylinder stands on the XY plane
        let (lo, hi) = roi_box(45.0, 60.0);
        assert_eq!(lo, Vec3::new(-45.0, -45.0, -ROI_BELOW_PLATTER_MM));
        assert_eq!(hi, Vec3::new(45.0, 45.0, 60.0));
    }

    #[test]
    fn the_rotation_shows_as_an_orientation() {
        // homing by a full turn leaves the table counter at 360: the
        // platter is back where it started
        assert!((wrapped(360.1) - 0.1).abs() < 1e-3);
        assert!((wrapped(-370.0) + 10.0).abs() < 1e-3);
        assert_eq!(wrapped(180.0), 180.0);
        assert_eq!(wrapped(-180.0), 180.0);
        assert!((wrapped(190.0) + 170.0).abs() < 1e-3);
        assert_eq!(deg(wrapped(720.0)), "0.0");
    }

    #[test]
    fn the_speed_is_slow_by_default_and_the_hint_follows_the_frame_rate() {
        assert!(TableUi::default().speed <= 10.0);
        // 8 °/s at 6.5 frames per second: 1.2° per frame, fine
        assert_eq!(too_fast(8.0, 6.5), None);
        // 20 °/s at 2.7 frames per second: 7.4° per frame
        let (per_frame, suggested) = too_fast(20.0, 2.7).unwrap();
        assert!(
            (per_frame - 7.4).abs() < 0.05 && suggested == 5.4,
            "{per_frame} {suggested}"
        );
        // no stream, no hint
        assert_eq!(too_fast(60.0, 0.0), None);
    }

    #[test]
    fn automatic_scan_parameters() {
        let mut ui = TableUi::default();
        assert_eq!(ui.capture, CAPTURE_STOP);
        assert_eq!(
            (ui.stop_step_deg, ui.frames_per_stop),
            (AUTO_STEP_DEG, AUTO_FRAMES_PER_STOP as f32)
        );
        // the quality defaults: the resolution of the core, finished, refined
        assert!((ui.resolution_mm() - AUTO_RESOLUTION_MM).abs() < 0.05);
        assert!(ui.finish && ui.refine);
        let quality = json!({
            "resolution_mm": ui.resolution_mm(), "finish": true, "refine": true,
        });
        // calibrated: stop and capture, the table moves at its own pace
        let p = ui.auto_params(vec![0.0, 20.0], true);
        assert_eq!(
            p,
            json!({
                "tilts_deg": [0.0, 20.0], "rotation_deg": 360.0,
                "mode": "stop-and-capture", "step_deg": AUTO_STEP_DEG,
                "frames_per_stop": AUTO_FRAMES_PER_STOP,
                "resolution_mm": quality["resolution_mm"], "finish": true, "refine": true,
            })
        );
        // not calibrated: only continuous turning works
        assert_eq!(
            ui.auto_params(vec![0.0], false),
            json!({
                "tilts_deg": [0.0], "rotation_deg": 360.0,
                "mode": "continuous", "deg_per_s": DEFAULT_SPEED_DEG_S,
                "resolution_mm": quality["resolution_mm"], "finish": true, "refine": true,
            })
        );
        // calibrated, continuous chosen
        ui.capture = 1;
        assert_eq!(ui.auto_params(vec![0.0], true)["mode"], "continuous");
        ui.capture = CAPTURE_STOP;
        ui.stop_step_deg = 15.0;
        ui.frames_per_stop = 4.2;
        let p = ui.auto_params(vec![0.0], true);
        assert_eq!(
            (&p["step_deg"], &p["frames_per_stop"]),
            (&json!(15.0), &json!(4))
        );
    }

    #[test]
    fn the_quality_choices_reach_the_automatic_scan() {
        // every choice is inside the range of `scan.reconstruct`
        assert!(
            MESH_RESOLUTIONS_MM
                .iter()
                .all(|mm| (0.05..=1.0).contains(mm))
        );
        let mut ui = TableUi {
            resolution: MESH_RESOLUTIONS_MM.len() - 1,
            finish: false,
            refine: false,
            ..TableUi::default()
        };
        let p = ui.auto_params(vec![0.0], true);
        assert_eq!(p["resolution_mm"], json!(0.3_f32));
        assert_eq!((&p["finish"], &p["refine"]), (&json!(false), &json!(false)));
        // an out-of-range choice falls back to the default of the core
        ui.resolution = 99;
        assert_eq!(ui.resolution_mm(), AUTO_RESOLUTION_MM);
    }

    #[test]
    fn a_job_names_its_phase_and_what_it_built() {
        let j = JobSnap::from_json(&json!({
            "kind": "auto", "state": "running", "step": 40, "steps": 43, "phase": "fill-holes",
            "reconstruction": {
                "resolution_mm": 0.1, "stops": 74, "refined": 70, "mean_rms_before_mm": 0.21,
                "mean_rms_mm": 0.09, "rejected_stops": 3, "rejected_passes": 1,
                "rescued_stops": 2, "warning": "scan-views-rejected",
                "pass_details": [
                    { "tilt_deg": -20.0, "stops": 20, "corrected_deg": 0.4, "corrected_mm": 0.3,
                      "rescued_stops": 2, "rejected_stops": 0, "rejected": false },
                    { "tilt_deg": 20.0, "stops": 3, "corrected_deg": 0.0, "corrected_mm": 0.0,
                      "rescued_stops": 0, "rejected_stops": 3, "rejected": true },
                ],
                "vertices": 90000, "faces": 180000, "seconds": 12.5,
            },
            "finish": {
                "islands_removed": 3, "nonmanifold_fixed": 12, "holes_filled": 5,
                "holes_left": 1, "faces_before": 180000, "faces_after": 90000,
                "max_error_mm": 0.02,
            },
        }))
        .unwrap();
        assert_eq!(
            j.phase.as_deref().and_then(phase_key),
            Some("scan-phase-fill-holes")
        );
        let r = j.reconstruction.unwrap();
        assert_eq!((r.stops, r.vertices, r.faces), (74, 90000, 180_000));
        assert_eq!((r.refined, r.mean_rms_before_mm), (70, Some(0.21)));
        assert_eq!(
            (r.rejected_stops, r.rejected_passes, r.rescued_stops),
            (3, 1, 2)
        );
        assert_eq!(r.warning.as_deref(), Some("scan-views-rejected"));
        assert_eq!(r.passes.len(), 2);
        assert!(!r.passes[0].rejected && r.passes[1].rejected);
        assert_eq!(
            (r.passes[0].tilt_deg, r.passes[0].rescued_stops),
            (-20.0, 2)
        );
        assert_eq!(r.passes[1].rejected_stops, 3);
        // a report of an older core has none of it
        let old = ReconSnap::from_json(&json!({ "stops": 5, "faces": 9 })).unwrap();
        assert_eq!(
            (old.rejected_stops, old.warning, old.passes.len()),
            (0, None, 0)
        );
        // the custom tilts start as the default ones of the core
        assert_eq!(tilts_text(&[-7.5, 0.0, 20.0]), "-7.5, 0, 20");
        assert_eq!(
            parse_tilts(&TableUi::default().custom, -30.0, 30.0),
            Some(AUTO_TILTS_DEG.to_vec())
        );
        assert_eq!(
            (r.resolution_mm, r.mean_rms_mm, r.seconds),
            (Some(0.1), Some(0.09), Some(12.5))
        );
        let f = j.finish.unwrap();
        assert_eq!((f.islands_removed, f.nonmanifold_fixed), (3, 12));
        assert_eq!((f.holes_filled, f.holes_left), (5, 1));
        assert_eq!((f.faces_before, f.faces_after), (180_000, 90_000));
        assert_eq!(f.max_error_mm, Some(0.02));
        // acquiring, a job of an older core and a phase of a newer one
        assert_eq!(phase_key("acquire"), None);
        assert_eq!(phase_key("later"), Some("scan-phase-other"));
        let old = JobSnap::from_json(&json!({ "state": "done", "steps": 3 })).unwrap();
        assert_eq!(
            (old.phase, old.reconstruction, old.finish),
            (None, None, None)
        );
    }

    #[test]
    fn what_a_job_reports() {
        let c = JobSnap::from_json(&json!({
            "kind": "calibrate", "state": "failed", "step": 24, "steps": 24,
            "error": "calibration-inaccurate",
            "calibration": {
                "rms_deg": 3.1, "rms_mm": 0.9, "pairs": 22, "pairs_expected": 24,
                "angle_gain": 0.947, "loop_verified": false,
            },
        }))
        .unwrap();
        let r = c.calibration.unwrap();
        assert_eq!((r.pairs, r.pairs_expected), (22, 24));
        assert_eq!(
            (r.rms_deg, r.rms_mm, r.angle_gain),
            (Some(3.1), Some(0.9), Some(0.947))
        );
        assert_eq!(c.quality, None);
        let a = JobSnap::from_json(&json!({
            "kind": "auto", "state": "done", "step": 4, "steps": 4,
            "quality": {
                "stops": 36, "flagged_stops": 3, "frames": 0, "fused": 36,
                "mean_rms_mm": 0.27, "loop_error_deg": [0.4, -1.2],
                "warning": "scan-quality-weak",
            },
        }))
        .unwrap();
        let q = a.quality.unwrap();
        assert_eq!((q.stops, q.flagged_stops, q.fused), (36, 3, 36));
        assert_eq!(q.mean_rms_mm, Some(0.27));
        assert_eq!(q.loop_error(), Some(1.2));
        assert_eq!(q.warning.as_deref(), Some("scan-quality-weak"));
        assert_eq!(
            warning_key("scan-quality-weak"),
            "scan-warn-scan-quality-weak"
        );
        assert_eq!(warning_key("other"), "scan-warn-other");
        assert_eq!(a.calibration, None);
        // a null report is no report
        let n = JobSnap::from_json(&json!({ "state": "done", "steps": 1, "quality": null }));
        assert_eq!(n.unwrap().quality, None);
    }

    /// Runs `id` on `popup` as an agent does and waits for its answer.
    fn run(popup: &mut crate::ScanPopup, id: &str, params: Value) -> Value {
        let ticket = popup.start_command(id, params).unwrap();
        let start = std::time::Instant::now();
        loop {
            popup.poll();
            if let Some(r) = popup.take_result(ticket) {
                return r.unwrap_or_else(|e| panic!("{id}: {e}"));
            }
            assert!(start.elapsed().as_secs() < 300, "{id} did not end");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// A default automatic scan (the parameters the window sends) on the
    /// simulated rig ends with the finished mesh in the view, its report in
    /// the window and the same mesh in the file handed to the host.
    #[test]
    fn a_default_automatic_scan_ends_with_the_finished_mesh_for_the_host() {
        use crate::{ScanPopup, ScanTarget};
        use dc_scanner::control;
        use dc_scanner::{RigJobState, ScanState};
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        let dir = tempfile::tempdir().unwrap();
        let mut popup = ScanPopup::new(None);
        popup.controller.lock().unwrap().set_rig_dir(dir.path());
        popup.set_target(ScanTarget::new([dc_case::ScanRole::Lower]));
        run(
            &mut popup,
            control::SCAN_CONNECT,
            json!({"backend": "sim", "options": {"realtime": false, "width": 200, "height": 125}}),
        );
        run(
            &mut popup,
            control::TURNTABLE_CONNECT,
            json!({"backend": "sim", "options": {"fast": true}}),
        );
        run(&mut popup, control::TURNTABLE_CALIBRATE, json!({}));
        let start = std::time::Instant::now();
        loop {
            popup.poll();
            let seen = popup.try_with(|c| {
                (
                    c.calibration_job().map(|j| j.state),
                    c.session().status().state,
                )
            });
            if let Some((Some(job), ScanState::Connected)) = seen
                && !matches!(job, RigJobState::Running | RigJobState::Paused)
            {
                break;
            }
            assert!(start.elapsed().as_secs() < 300, "calibration: {seen:?}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(popup.try_with(|c| c.calibration().is_some()), Some(true));

        // the window's own parameters, the defaults of its controls
        let mut params = TableUi::default().auto_params(vec![0.0], true);
        params["settle_s"] = json!(0);
        params["deg_per_s"] = json!(90);
        // a debug build of the simulated scan is slow at the 0.1 mm default
        params["resolution_mm"] = json!(0.5);
        let mut h = Harness::builder()
            .with_size(vec2(1600.0, 1000.0))
            // the device pickers are the window's own ComboBoxes
            .with_accessibility_check(false)
            .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
            .build_ui_state(|ui, p: &mut ScanPopup| p.show(ui), popup);
        h.state_mut().command(control::SCAN_AUTO, params);
        let start = std::time::Instant::now();
        let snap = loop {
            h.step();
            let p = h.state();
            if let Some(s) = &p.ui.snap
                && let Some(a) = &s.auto
                && !a.active
                && !p.ui.auto_pending
                && !s.live()
            {
                assert!(
                    a.error.is_none(),
                    "the automatic scan failed: {:?}",
                    a.error
                );
                break s.clone();
            }
            assert!(
                start.elapsed().as_secs() < 600,
                "the automatic scan did not end"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let job = snap.auto.clone().unwrap();
        let (r, f) = (
            job.reconstruction.expect("a reconstruction report"),
            job.finish.expect("a finish report"),
        );
        assert!(r.refined > 0 && r.stops > 0 && r.faces > 0, "{r:?}");
        assert!((r.resolution_mm.unwrap() - 0.5).abs() < 1e-3, "{r:?}");
        assert!(
            f.faces_after > 0 && f.faces_after <= f.faces_before,
            "{f:?}"
        );
        let mesh = snap.mesh.as_ref().expect("the finished mesh");
        assert_eq!(mesh.face_count() as u64, f.faces_after);
        // the view shows the mesh and the window its report
        assert!(h.state().ui.show_surface);
        h.step();
        assert!(h.query_by_label_contains("before optimising").is_some());
        assert!(h.query_by_label_contains("stops joined").is_some());

        // the file for the host is that mesh
        let ticket = h
            .state_mut()
            .start_command(control::SCAN_IMPORT, json!({ "role": "lower" }))
            .unwrap();
        let start = std::time::Instant::now();
        let mut faces = None;
        while h.state_mut().take_result(ticket).is_none() {
            h.step();
            for out in h.state_mut().take_outputs() {
                faces = Some(dc_mesh::io::read(out.path()).unwrap().face_count());
                h.state_mut().output_done(out.id(), Ok(json!({})));
            }
            assert!(start.elapsed().as_secs() < 300, "scan.import did not end");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(faces, Some(mesh.face_count()));
    }

    #[test]
    fn a_job_reports_reconnections_and_kept_stops() {
        let j = JobSnap::from_json(&json!({
            "kind": "auto", "state": "running", "step": 3, "steps": 20, "phase": "acquire",
            "reconnects": 2, "reconnecting": true, "kept_stops": null,
        }))
        .unwrap();
        assert_eq!(
            (j.reconnects, j.reconnecting, j.kept_stops),
            (2, true, None)
        );
        let failed = JobSnap::from_json(&json!({
            "kind": "auto", "state": "failed", "step": 5, "steps": 20,
            "error": "ble-disconnected", "kept_stops": 4,
        }))
        .unwrap();
        assert_eq!((failed.kept_stops, failed.reconnects), (Some(4), 0));
        assert!(!failed.reconnecting && failed.error.is_some());
        // the link dropped: the table is known, not connected
        let t = TableSnap::from_json(&json!({
            "connected": false,
            "info": { "backend": "sim", "id": "sim-turntable", "name": "Sim" },
        }));
        assert!(!t.connected && t.info.is_some());
    }

    #[test]
    fn the_finish_phases_show_the_phase_only() {
        let running = |phase: &str| {
            JobSnap::from_json(
                &json!({ "state": "running", "step": 9, "steps": 10, "phase": phase }),
            )
            .unwrap()
        };
        assert!(!running("acquire").finishing());
        for phase in [
            "refine",
            "fuse",
            "extract",
            "clean",
            "fill-holes",
            "optimise",
        ] {
            assert!(running(phase).finishing(), "{phase}");
        }
        let ended = JobSnap::from_json(
            &json!({ "state": "done", "step": 10, "steps": 10, "phase": "optimise" }),
        )
        .unwrap();
        assert!(!ended.finishing());
        let mut s = Snapshot::read(&dc_scanner::ScanController::new());
        assert!(!s.finishing() && !s.reconnecting());
        s.auto = Some(running("fuse"));
        assert!(s.finishing() && !s.reconnecting());
        s.auto = Some(running("acquire"));
        assert!(!s.finishing());
        // the acquisition is stopped while the job still runs: processing
        s.status.state = ScanState::Stopped;
        assert!(s.finishing());
        s.status.state = ScanState::Scanning;
        assert!(!s.finishing());
        s.auto = Some(JobSnap::from_json(&json!({ "state": "failed", "steps": 5 })).unwrap());
        s.status.state = ScanState::Stopped;
        assert!(!s.finishing());
        s.auto = None;
        s.calibrating = Some(
            JobSnap::from_json(&json!({ "state": "running", "steps": 24, "reconnecting": true }))
                .unwrap(),
        );
        assert!(s.reconnecting() && !s.finishing());
    }

    /// A popup on the simulated rig with its true calibration stored in
    /// `dir`, so that stop and capture runs without measuring it.
    fn rig_popup(dir: &std::path::Path) -> crate::ScanPopup {
        use dc_scanner::rig::{RigCalibration, rfc3339, save_calibration};
        use dc_scanner::sim::{self, SimOptions};
        let options = SimOptions {
            realtime: false,
            width: 120,
            height: 75,
            ..SimOptions::default()
        };
        let to_cam = sim::rig_camera_pose(&options).inverse();
        let axis_dir = to_cam.transform_vector3(Vec3::Z).normalize();
        let calibration = RigCalibration {
            axis_point: to_cam.transform_point3(Vec3::ZERO).into(),
            axis_dir: axis_dir.into(),
            platter_up: axis_dir.into(),
            tilt_axis_point: Some(
                to_cam
                    .transform_point3(Vec3::new(0.0, 0.0, -sim::RIG_TILT_PIVOT_MM))
                    .into(),
            ),
            tilt_axis_dir: Some(to_cam.transform_vector3(Vec3::X).normalize().into()),
            scanner_serial: None,
            turntable_id: "sim-turntable".into(),
            created: rfc3339(std::time::SystemTime::now()),
            fit_rms_deg: 0.0,
            fit_rms_mm: 0.0,
            angle_gain: 1.0,
            fit_pairs: 24,
            imu_up: None,
        };
        save_calibration(dir, &calibration).unwrap();
        let mut popup = crate::ScanPopup::new(None);
        popup.controller.lock().unwrap().set_rig_dir(dir);
        run(
            &mut popup,
            dc_scanner::control::SCAN_CONNECT,
            json!({"backend": "sim", "options": {"realtime": false, "width": 120, "height": 75}}),
        );
        let r = run(
            &mut popup,
            dc_scanner::control::TURNTABLE_CONNECT,
            json!({"backend": "sim", "options": {"fast": true}}),
        );
        assert_eq!(r["calibrated"], true, "{r}");
        popup
    }

    /// The window of `popup`, drawn headless.
    fn window(popup: crate::ScanPopup) -> egui_kittest::Harness<'static, crate::ScanPopup> {
        egui_kittest::Harness::builder()
            .with_size(vec2(1600.0, 1000.0))
            .with_accessibility_check(false)
            .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
            .build_ui_state(|ui, p: &mut crate::ScanPopup| p.show(ui), popup)
    }

    /// One pass of ten stops 10° apart, no finish: the scan ends paused
    /// or, when it fails, stopped with its stops.
    fn quick_scan() -> Value {
        json!({"mode": "stop-and-capture", "tilts_deg": [0], "rotation_deg": 90, "step_deg": 10,
               "frames_per_stop": 1, "deg_per_s": 90, "settle_s": 0, "finish": false})
    }

    /// Draws the window until the automatic scan ended and the popup
    /// finalised it; returns what the window then reads.
    fn until_the_scan_ends(h: &mut egui_kittest::Harness<'static, crate::ScanPopup>) -> Snapshot {
        let start = std::time::Instant::now();
        loop {
            h.step();
            if let Some(s) = &h.state().ui.snap
                && s.auto.as_ref().is_some_and(|a| !a.active)
                && !h.state().is_busy()
            {
                break;
            }
            assert!(start.elapsed().as_secs() < 300, "the scan did not end");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // the job is finalised on the command thread, then drawn
        for _ in 0..10 {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        h.state().ui.snap.clone().unwrap()
    }

    #[test]
    fn a_link_that_does_not_come_back_shows_the_lost_table_and_the_kept_stops() {
        use dc_scanner::control::{SCAN_AUTO, SCAN_RECONSTRUCT};
        use egui::Role;
        use egui_kittest::kittest::Queryable;
        let dir = tempfile::tempdir().unwrap();
        let popup = rig_popup(dir.path());
        // the link drops on the move to the fourth stop and does not return
        popup
            .controller
            .lock()
            .unwrap()
            .sim_rig()
            .inject_link_drop(5, false);
        let mut h = window(popup);
        h.state_mut().command(SCAN_AUTO, quick_scan());
        let snap = until_the_scan_ends(&mut h);
        let job = snap.auto.clone().unwrap();
        assert_eq!(
            job.error.as_ref().map(|e| e.code.as_str()),
            Some("ble-disconnected")
        );
        assert_eq!(job.kept_stops, Some(4));
        assert!(!snap.table.connected && snap.table.info.is_some());
        assert_eq!(snap.status.state, dc_scanner::ScanState::Stopped);
        assert!(snap.cloud.is_some());
        // the header: lost, with a way to connect it again, not "Ready"
        assert!(h.query_by_label("Link lost").is_some());
        assert!(
            h.query_by_role_and_label(Role::Button, "Connect the turntable")
                .is_some()
        );
        assert!(
            h.query_by_role_and_label(Role::Button, "Disconnect the turntable")
                .is_none()
        );
        assert!(h.query_by_label("Ready").is_none());
        assert!(h.query_by_label("Reconnecting").is_none());
        // the stops taken are kept, and the window says so
        assert!(h.query_by_label_contains("4 stops kept").is_some());
        // the error says they are kept
        assert!(t(&job.error.unwrap().key()).contains("The stops taken so far are kept"));
        // and they build a surface
        let r = {
            let ticket = h
                .state_mut()
                .start_command(
                    SCAN_RECONSTRUCT,
                    json!({"resolution_mm": 0.5, "refine": false}),
                )
                .unwrap();
            let start = std::time::Instant::now();
            loop {
                h.step();
                if let Some(r) = h.state_mut().take_result(ticket) {
                    break r.unwrap();
                }
                assert!(start.elapsed().as_secs() < 300, "rebuild did not end");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        assert_eq!(r["reconstruction"]["stops"], 4, "{r}");
        assert!(r["faces"].as_u64().unwrap() > 0, "{r}");
    }

    #[test]
    fn a_link_that_comes_back_is_a_warning_and_the_scan_goes_on() {
        use dc_scanner::control::SCAN_AUTO;
        use egui_kittest::kittest::Queryable;
        let dir = tempfile::tempdir().unwrap();
        let popup = rig_popup(dir.path());
        popup
            .controller
            .lock()
            .unwrap()
            .sim_rig()
            .inject_link_drop(5, true);
        let mut h = window(popup);
        h.state_mut().command(SCAN_AUTO, quick_scan());
        let snap = until_the_scan_ends(&mut h);
        let job = snap.auto.clone().unwrap();
        assert!(job.error.is_none(), "{:?}", job.error);
        assert_eq!((job.reconnects, job.reconnecting), (1, false));
        assert_eq!(job.quality.as_ref().map(|q| q.stops), Some(10));
        assert!(snap.table.connected);
        assert!(
            h.query_by_label_contains(
                "disconnected 1 times and was reconnected; the scan went on."
            )
            .is_some()
        );
        // no red banner, no lost link
        assert!(h.query_by_label("Link lost").is_none());
        assert!(h.query_by_label("Ready").is_some());
    }
}
