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
use dc_scanner::rig::ROI_BELOW_PLATTER_MM;
use dc_ui::fonts;
use dc_ui::icons as ic;
use dc_ui::theme::colors;
use dc_ui::widgets::{self, BadgeKind, Button};
use egui::{Align, Layout, RichText, Sense, Stroke, Ui, vec2};
use glam::{Quat, Vec3};
use serde_json::{Value, json};

use super::{Act, ScanUi, Snapshot};
use crate::{ScanFailure, caption, t, tr_n};

/// Seconds the Refresh button looks for turntables (Bluetooth ones answer
/// within a few seconds of their advertising).
pub(super) const SEARCH_S: f32 = 5.0;
/// Seconds the popup looks for turntables when it opens.
pub(crate) const OPEN_SEARCH_S: f32 = 1.0;

/// Tilts of the presets of the automatic scan: Arch, Quick.
const PRESETS: [&[f32]; 2] = [&[-20.0, 0.0, 20.0], &[0.0]];
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

/// A vector of three numbers.
fn vec3(v: &Value) -> Option<Vec3> {
    let a = v.as_array()?;
    match a.as_slice() {
        [x, y, z] => Some(Vec3::new(num(x)?, num(y)?, num(z)?)),
        _ => None,
    }
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
        })
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

/// The stored calibration of the rig (`turntable.calibration`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct CalSnap {
    pub turntable_id: Option<String>,
    pub scanner_serial: Option<String>,
    /// When it was made, in seconds since 1970.
    pub created: Option<u64>,
    /// A point of the axis on the platter, and the platter's normal
    /// towards the scanner (camera frame, mm).
    pub axis_point: Option<Vec3>,
    pub platter_up: Option<Vec3>,
    /// A point and the direction of the tilt axis, when the table tilts.
    pub tilt_axis: Option<(Vec3, Vec3)>,
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
            axis_point: vec3(&v["axis_point"]),
            platter_up: vec3(&v["platter_up"]),
            tilt_axis: vec3(&v["tilt_axis_point"]).zip(vec3(&v["tilt_axis_dir"])),
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

    /// Box around the cylinder of the region of interest (`radius_mm`
    /// around the axis, from the platter to `height_mm` above it) in the
    /// frame of a scan that began with the table tilted by `tilt_deg`.
    pub fn roi_box(&self, radius_mm: f32, height_mm: f32, tilt_deg: f32) -> Option<(Vec3, Vec3)> {
        const AROUND: usize = 32;
        let (point, up) = (self.axis_point?, self.platter_up?.try_normalize()?);
        let e1 = up.any_orthonormal_vector();
        let e2 = up.cross(e1);
        let tilt = self.tilt_axis.and_then(|(p, d)| {
            Some((
                p,
                Quat::from_axis_angle(d.try_normalize()?, tilt_deg.to_radians()),
            ))
        });
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for h in [-ROI_BELOW_PLATTER_MM, height_mm] {
            for i in 0..AROUND {
                let a = std::f32::consts::TAU * i as f32 / AROUND as f32;
                let mut p = point + up * h + (e1 * a.cos() + e2 * a.sin()) * radius_mm;
                if let Some((axis_point, q)) = tilt {
                    p = axis_point + q * (p - axis_point);
                }
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        (lo.is_finite() && hi.is_finite()).then_some((lo, hi))
    }
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
/// Degrees between stops and frames taken at each stop, as `scan.auto`
/// defaults to.
const DEFAULT_STOP_STEP_DEG: f32 = 10.0;
const DEFAULT_FRAMES_PER_STOP: f32 = 3.0;

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
fn deg(v: f32) -> String {
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
            custom: "-20, 0, 20".to_owned(),
            rotation_deg: 360.0,
            expanded: true,
            confirm_auto: None,
            focus_calibrate: false,
            capture: CAPTURE_STOP,
            stop_step_deg: DEFAULT_STOP_STEP_DEG,
            frames_per_stop: DEFAULT_FRAMES_PER_STOP,
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
                1.0,
                0,
                "°",
                DEFAULT_STOP_STEP_DEG,
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
                DEFAULT_FRAMES_PER_STOP,
            );
            ui.add_space(4.0);
            caption(ui, t("scan-auto-stop-hint"));
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
fn sub_heading(ui: &mut Ui, key: &str) {
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

/// How the latest automatic scan went: the views fused and flagged, the
/// residual, the loop closure error and the warning of a weak scan.
fn quality_report(ui: &mut Ui, q: &QualityReport) {
    let c = *colors(ui);
    ui.add_space(8.0);
    sub_heading(ui, "scan-quality-title");
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
    if let Some(mm) = q.mean_rms_mm {
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
    if let Some(code) = &q.warning {
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{} {}", ic::WARNING, t(warning_key(code))))
                .font(fonts::ui(12.0))
                .color(c.status_warning),
        );
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

        // picker, refresh, connection
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_enabled_ui(!table.connected && !busy, |ui| {
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
                let params = self.table.devices.get(self.table.device).map_or(
                    Value::Null,
                    |d| json!({ "backend": d.backend, "device": d.id }),
                );
                if Button::secondary(t("turntable-connect"))
                    .icon(ic::PLUGS_CONNECTED)
                    .enabled(!busy && !self.table.devices.is_empty())
                    .show(ui)
                    .clicked()
                {
                    out.push(Act::Run(TURNTABLE_CONNECT, params));
                }
            }
            // a paused automatic scan holds the turntable still
            let paused = snap.auto.as_ref().is_some_and(|j| j.paused);
            let (kind, key) = match (table.connected, table.busy && !paused) {
                (false, _) => (BadgeKind::Neutral, "turntable-state-off"),
                (true, false) => (BadgeKind::Info, "turntable-state-ready"),
                (true, true) => (BadgeKind::Warning, "turntable-state-moving"),
            };
            widgets::badge(ui, kind, t(key));
        });
        if !table.connected {
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
                text(ui, tr_n("turntable-rotation", deg(r)));
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
                progress(
                    ui,
                    &tr_n(
                        "turntable-cal-progress",
                        format!("{} / {}", j.step, j.steps),
                    ),
                    j,
                );
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
            progress(
                ui,
                &tr_n(
                    "turntable-cal-progress",
                    format!("{} / {}", j.step, j.steps),
                ),
                j,
            );
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
        if !table.connected {
            caption(ui, t("scan-auto-needs"));
            return;
        }
        if let Some(a) = JobSnap::running(snap.auto.as_ref()) {
            let text = dc_i18n::tr_args(
                "scan-auto-progress",
                &[
                    ("step", &a.step.to_string()),
                    ("steps", &a.steps.to_string()),
                    ("tilt", &deg(a.tilt_deg.unwrap_or(0.0))),
                ],
            );
            progress(ui, &text, a);
            ui.add_space(4.0);
            caption(ui, t("scan-auto-hint"));
            return;
        }
        let busy = busy || JobSnap::running(snap.calibrating.as_ref()).is_some();
        let range = table.tilt_range();
        let can = matches!(snap.status.state, ScanState::Connected | ScanState::Stopped) && !busy;
        let tilts = ui
            .add_enabled_ui(can, |ui| {
                if table.tilt.is_some() {
                    let labels = [
                        t("scan-auto-arch"),
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
                        "-20, 0, 20",
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
        if !busy && let Some(q) = snap.auto.as_ref().and_then(|j| j.quality.as_ref()) {
            quality_report(ui, q);
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
        // the platter faces the scanner: its normal is -Y here, the axis
        // is 200 mm away along Z
        let (lo, hi) = calibration(json!({})).roi_box(45.0, 60.0, 0.0).unwrap();
        assert!(
            (lo.x + 45.0).abs() < 0.1 && (hi.x - 45.0).abs() < 0.1,
            "{lo} {hi}"
        );
        assert!((lo.z - 155.0).abs() < 0.1 && (hi.z - 245.0).abs() < 0.1);
        assert!((hi.y - ROI_BELOW_PLATTER_MM).abs() < 1e-4 && (lo.y + 60.0).abs() < 1e-4);
        // tilting the table about the X axis through the axis point moves
        // the cylinder with it
        let tilted = calibration(json!({
            "tilt_axis_point": [0.0, 0.0, 200.0],
            "tilt_axis_dir": [1.0, 0.0, 0.0],
        }));
        let (tlo, thi) = tilted.roi_box(45.0, 60.0, 90.0).unwrap();
        assert!((tlo.x + 45.0).abs() < 0.1 && (thi.x - 45.0).abs() < 0.1);
        // the platter normal -Y turned to -Z: the height runs from Z 201 to
        // Z 140 and the disk lies in the XY plane
        assert!(
            (tlo.z - 140.0).abs() < 0.1 && (thi.z - 201.0).abs() < 0.1,
            "{tlo} {thi}"
        );
        assert!((thi.y - 45.0).abs() < 0.1 && (tlo.y + 45.0).abs() < 0.1);
        let flat = tilted.roi_box(45.0, 60.0, 0.0).unwrap();
        assert_eq!(flat, (lo, hi), "no tilt, no move");
        // without an axis there is no box
        let bare = CalSnap::from_json(&json!({ "turntable_id": "t" })).unwrap();
        assert_eq!(bare.roi_box(45.0, 60.0, 0.0), None);
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
        assert_eq!((ui.stop_step_deg, ui.frames_per_stop), (10.0, 3.0));
        // calibrated: stop and capture, the table moves at its own pace
        let p = ui.auto_params(vec![0.0, 20.0], true);
        assert_eq!(
            p,
            json!({
                "tilts_deg": [0.0, 20.0], "rotation_deg": 360.0,
                "mode": "stop-and-capture", "step_deg": 10.0, "frames_per_stop": 3,
            })
        );
        // not calibrated: only continuous turning works
        assert_eq!(
            ui.auto_params(vec![0.0], false),
            json!({
                "tilts_deg": [0.0], "rotation_deg": 360.0,
                "mode": "continuous", "deg_per_s": DEFAULT_SPEED_DEG_S,
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
}
