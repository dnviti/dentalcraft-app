//! "Arch line", a manual correction of the Final position (the plugin's
//! `oa.arch_line_show` / `_edit` / `_apply` / `_reset` / `_hide`,
//! `case_plan.show_arch_line`, `read_arch_line`, `store_arch_line`,
//! `reset_arch_line`).
//!
//! - The line shown for an arch is the one the final position aligns the
//!   prominent cusps on: the line drawn by the clinician
//!   (`Setup::drawn`), else the line of the last computation
//!   (`Setup::lines`). It is drawn over the 3D view in front of the
//!   teeth, for every arch shown, as a smooth curve through
//!   [`CONTROLS`] points spread evenly along it (the plugin's
//!   `ARCH_LINE_POINTS`), with the teeth's final position ghosted.
//! - A point is dragged with the left button on the occlusal plane (the
//!   horizontal plane of the world frame through the point; seen edge-on,
//!   on the plane of the screen). The points moved are kept in the tool,
//!   not in the case, until Apply.
//! - Apply sends the curve through the points, [`SAMPLES`] points per
//!   segment as the plugin reads its Bézier curve back (automatic handles,
//!   `interpolate_bezier`), to `aligners.planning.setup.line`: one undo
//!   step that stores the line and, with "Compute the final position
//!   again" (default, as the plugin's Apply), computes the final position
//!   on it. "Back to the automatic line" drops the drawn line and computes
//!   again; with only moved points, it forgets them. Hide and Show only
//!   change what the view draws.
//! - The moved points of an arch are forgotten when its line in force
//!   changes (applied, undone, computed again).

use std::collections::BTreeMap;

use dc_case::{CaseState, ScanRole};
use dc_ui::theme::colors;
use dc_ui::widgets::Button;
use dc_viewport::{Viewport, ViewportResponse};
use egui::{Color32, PointerButton, Pos2, Rect, RichText, Stroke, Ui};
use glam::{DVec3, Vec3};
use serde_json::{Value, json};

use crate::manual::Tool;
use crate::overlay;
use crate::t;
use crate::workspace::Workspace;

/// The command of the tool.
pub(crate) const LINE: &str = "aligners.planning.setup.line";
/// Points of the line that can be moved (the plugin's `ARCH_LINE_POINTS`).
pub(crate) const CONTROLS: usize = 11;
/// Points of every segment of the curve sent (`ARCH_LINE_SAMPLES`).
pub(crate) const SAMPLES: usize = 24;
/// Distance on the screen within which a press takes a point (points).
const GRAB_RADIUS: f32 = 10.0;
/// Width of the line on the screen (points).
const LINE_WIDTH: f32 = 2.5;
/// Radius of a point on the screen (points).
const POINT_RADIUS: f32 = 5.0;
/// Points of every segment of the curve drawn.
const DRAWN_SAMPLES: usize = 12;

/// The points moved in an arch and the line in force they were taken from.
#[derive(Debug, Clone, PartialEq)]
struct Edit {
    from: Vec<DVec3>,
    controls: Vec<DVec3>,
}

/// State of the tool.
#[derive(Debug, Clone)]
pub(crate) struct ArchLine {
    /// The line is not drawn (Hide).
    pub hidden: bool,
    /// The arch Apply and "Back to the automatic line" act on.
    pub arch: ScanRole,
    /// Apply computes the final position again.
    pub recompute: bool,
    /// Points moved and not applied, per arch (case coordinates).
    edits: BTreeMap<ScanRole, Edit>,
    /// The point being dragged.
    drag: Option<(ScanRole, usize)>,
}

impl Default for ArchLine {
    fn default() -> Self {
        Self {
            hidden: false,
            arch: ScanRole::Lower,
            recompute: true,
            edits: BTreeMap::new(),
            drag: None,
        }
    }
}

/// The line in force for `role` (case coordinates) and whether it is the
/// one drawn by the clinician; `None` when the arch has none yet.
pub(crate) fn line_of(state: &CaseState, role: ScanRole) -> Option<(&[DVec3], bool)> {
    let s = &overlay::aligners(state)?.setup;
    match s.drawn.get(&role) {
        Some(l) if l.len() >= 2 => Some((l, true)),
        _ => s
            .lines
            .get(&role)
            .filter(|l| l.len() >= 2)
            .map(|l| (l.as_slice(), false)),
    }
}

/// The arches that have a line, upper first.
pub(crate) fn arches_with_line(state: &CaseState) -> Vec<ScanRole> {
    [ScanRole::Upper, ScanRole::Lower]
        .into_iter()
        .filter(|r| line_of(state, *r).is_some())
        .collect()
}

/// `n` points evenly spaced along the polyline `p` (the plugin's
/// `_resample`).
pub(crate) fn resample(p: &[DVec3], n: usize) -> Vec<DVec3> {
    if p.len() < 2 || n < 2 {
        return p.to_vec();
    }
    let mut acc = vec![0.0];
    for w in p.windows(2) {
        acc.push(acc[acc.len() - 1] + w[0].distance(w[1]));
    }
    let total = acc[acc.len() - 1];
    let mut k = 0;
    (0..n)
        .map(|i| {
            let u = total * i as f64 / (n - 1) as f64;
            while k + 2 < acc.len() && acc[k + 1] < u {
                k += 1;
            }
            let span = acc[k + 1] - acc[k];
            let f = if span > 0.0 {
                ((u - acc[k]) / span).clamp(0.0, 1.0)
            } else {
                0.0
            };
            p[k].lerp(p[k + 1], f)
        })
        .collect()
}

/// The automatic handles of the points `p` (left, right), as Blender
/// computes them for an open Bézier curve (`calchandleNurb_intern`,
/// `HD_AUTO`): along the sum of the unit directions to the neighbours, the
/// ends mirrored.
fn auto_handles(p: &[DVec3]) -> Vec<(DVec3, DVec3)> {
    let n = p.len();
    (0..n)
        .map(|i| {
            let p2 = p[i];
            let (p1, p3) = match (i.checked_sub(1).map(|j| p[j]), p.get(i + 1).copied()) {
                (Some(a), Some(b)) => (a, b),
                (None, Some(b)) => (2.0 * p2 - b, b),
                (Some(a), None) => (a, 2.0 * p2 - a),
                (None, None) => (p2, p2),
            };
            let (da, db) = (p2 - p1, p3 - p2);
            let la = match da.length() {
                l if l > 0.0 => l,
                _ => 1.0,
            };
            let lb = match db.length() {
                l if l > 0.0 => l,
                _ => 1.0,
            };
            let tvec = db / lb + da / la;
            let len = tvec.length() * 2.5614;
            if len == 0.0 {
                return (p2, p2);
            }
            (p2 - tvec * (la / len), p2 + tvec * (lb / len))
        })
        .collect()
}

/// The cubic Bézier `a, h1, h2, b` at `u`.
fn bezier(a: DVec3, h1: DVec3, h2: DVec3, b: DVec3, u: f64) -> DVec3 {
    let v = 1.0 - u;
    a * (v * v * v) + h1 * (3.0 * v * v * u) + h2 * (3.0 * v * u * u) + b * (u * u * u)
}

/// The smooth curve through `controls` with `samples` points per segment
/// (the last one is the next segment's first), as the plugin reads it back
/// (`read_arch_line`: `interpolate_bezier` with the automatic handles).
fn curve_with(controls: &[DVec3], samples: usize) -> Vec<DVec3> {
    if controls.len() < 2 || samples < 2 {
        return controls.to_vec();
    }
    let h = auto_handles(controls);
    let mut out = Vec::with_capacity((samples - 1) * (controls.len() - 1) + 1);
    for i in 0..controls.len() - 1 {
        let (a, b) = (controls[i], controls[i + 1]);
        for k in 0..samples - 1 {
            let u = k as f64 / (samples - 1) as f64;
            out.push(bezier(a, h[i].1, h[i + 1].0, b, u));
        }
    }
    out.push(controls[controls.len() - 1]);
    out
}

/// The line sent by Apply: the curve through `controls`.
pub(crate) fn curve(controls: &[DVec3]) -> Vec<DVec3> {
    curve_with(controls, SAMPLES)
}

fn arch_name(role: ScanRole) -> &'static str {
    if role == ScanRole::Upper {
        "upper"
    } else {
        "lower"
    }
}

/// A point of the line moved to where the ray from the pointer meets the
/// occlusal plane through it (world coordinates, `Z` up); seen edge-on,
/// the plane of the screen through it (`forward` the view direction).
pub(crate) fn dragged_to(point: Vec3, origin: Vec3, dir: Vec3, forward: Vec3) -> Option<Vec3> {
    let normal = if dir.z.abs() > 0.1 { Vec3::Z } else { forward };
    let den = dir.dot(normal);
    if den.abs() < 1e-6 {
        return None;
    }
    let s = (point - origin).dot(normal) / den;
    (s > 0.0).then(|| origin + dir * s)
}

impl ArchLine {
    /// Forgets the moved points of an arch whose line in force changed
    /// since (applied, undone, computed again, gone).
    pub(crate) fn follow(&mut self, state: &CaseState) {
        self.edits
            .retain(|role, e| line_of(state, *role).is_some_and(|(l, _)| l == e.from.as_slice()));
        if self
            .drag
            .is_some_and(|(role, _)| line_of(state, role).is_none())
        {
            self.drag = None;
        }
    }

    /// The points of the line of `role` shown (case coordinates): the moved
    /// ones, else spread along the line in force.
    pub(crate) fn controls(&self, state: &CaseState, role: ScanRole) -> Option<Vec<DVec3>> {
        let (line, _) = line_of(state, role)?;
        match self.edits.get(&role) {
            Some(e) if e.from.as_slice() == line => Some(e.controls.clone()),
            _ => Some(resample(line, CONTROLS)),
        }
    }

    /// Points of `role` were moved and not applied.
    pub(crate) fn pending(&self, state: &CaseState, role: ScanRole) -> bool {
        match (self.edits.get(&role), line_of(state, role)) {
            (Some(e), Some((line, _))) => {
                e.from.as_slice() == line && e.controls != resample(line, CONTROLS)
            }
            _ => false,
        }
    }

    /// Moves point `index` of the line of `role` to `to` (case
    /// coordinates). `false` when the arch has no line or no such point.
    pub(crate) fn move_point(
        &mut self,
        state: &CaseState,
        role: ScanRole,
        index: usize,
        to: DVec3,
    ) -> bool {
        let Some(mut controls) = self.controls(state, role) else {
            return false;
        };
        let Some(p) = controls.get_mut(index) else {
            return false;
        };
        *p = to;
        let from = line_of(state, role)
            .map(|(l, _)| l.to_vec())
            .unwrap_or_default();
        self.edits.insert(role, Edit { from, controls });
        self.arch = role;
        true
    }

    /// The parameters of Apply for `role`: the curve through its points.
    pub(crate) fn apply_params(&self, state: &CaseState, role: ScanRole) -> Option<Value> {
        let points: Vec<[f64; 3]> = curve(&self.controls(state, role)?)
            .into_iter()
            .map(|p| p.to_array().map(|c| (c * 1e4).round() / 1e4))
            .collect();
        Some(json!({
            "arch": arch_name(role),
            "points": points,
            "recompute": self.recompute,
        }))
    }
}

/// The parameters of "Back to the automatic line" for `role`.
pub(crate) fn reset_params(role: ScanRole) -> Value {
    json!({ "arch": arch_name(role), "recompute": true })
}

/// The notice after the command ran.
pub(crate) fn feedback_key(out: &Value) -> &'static str {
    match (out["line"].as_str(), out["recomputed"].as_bool()) {
        (Some("automatic"), _) => "ws-archline-done-auto",
        (_, Some(false)) => "ws-archline-done-stored",
        _ => "ws-archline-done",
    }
}

/// What the viewport needs to draw and drag the lines.
pub(crate) struct LineView<'a> {
    pub state: &'a CaseState,
    /// The arches shown.
    pub upper: bool,
    pub lower: bool,
    pub color: Color32,
    pub halo: Color32,
}

/// Draws the lines of the arches shown over the viewport and drags their
/// points. Called while the tool is open and the line not hidden.
pub(crate) fn viewport(
    al: &mut ArchLine,
    ui: &Ui,
    vp: &Viewport,
    vr: &ViewportResponse,
    v: &LineView<'_>,
) {
    let rect: Rect = vr.response.rect;
    let world = v.state.case.world_frame.from_case;
    let cam = vp.camera();
    let eye = cam.eye();
    let fwd = cam.forward();
    let to_world = |p: DVec3| world.apply_point(p).as_vec3();
    let on_screen = |w: Vec3| (fwd.dot(w - eye) > 0.0).then(|| vp.screen_pos(rect, w));
    let roles: Vec<ScanRole> = arches_with_line(v.state)
        .into_iter()
        .filter(|r| {
            if *r == ScanRole::Upper {
                v.upper
            } else {
                v.lower
            }
        })
        .collect();
    let lines: Vec<(ScanRole, Vec<DVec3>)> = roles
        .iter()
        .filter_map(|r| Some((*r, al.controls(v.state, *r)?)))
        .collect();

    // a press near a point takes it; the drag moves it on the occlusal plane
    let primary = PointerButton::Primary;
    let nearest = |pos: Pos2| -> Option<(ScanRole, usize)> {
        lines
            .iter()
            .flat_map(|(r, c)| c.iter().enumerate().map(move |(i, p)| (*r, i, *p)))
            .filter_map(|(r, i, p)| Some((r, i, on_screen(to_world(p))?.distance(pos))))
            .filter(|(_, _, d)| *d <= GRAB_RADIUS)
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(r, i, _)| (r, i))
    };
    if al.drag.is_none()
        && vr.response.drag_started_by(primary)
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        al.drag = nearest(origin);
    }
    if let Some((role, i)) = al.drag {
        let pointer = vr.response.interact_pointer_pos();
        let point = lines
            .iter()
            .find(|(r, _)| *r == role)
            .and_then(|(_, c)| c.get(i))
            .map(|p| to_world(*p));
        if let (Some(pos), Some(point)) = (pointer, point) {
            let ray = vp.ray_at(rect, pos);
            if let Some(w) = dragged_to(point, ray.origin, ray.dir, fwd) {
                let case = world.inverse().apply_point(w.as_dvec3());
                al.move_point(v.state, role, i, case);
            }
        }
        if vr.response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            al.drag = None;
        }
    }
    let hot = al
        .drag
        .or_else(|| vr.response.hover_pos().and_then(nearest));
    if hot.is_some() {
        ui.ctx().set_cursor_icon(if al.drag.is_some() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
    }

    // the curve, in front of the teeth, and its points
    let painter = ui.painter_at(rect);
    let halo = Stroke::new(LINE_WIDTH + 2.0, v.halo);
    let line = Stroke::new(LINE_WIDTH, v.color);
    for (role, controls) in &lines {
        let controls = al
            .controls(v.state, *role)
            .unwrap_or_else(|| controls.clone());
        let pts: Vec<Option<Pos2>> = curve_with(&controls, DRAWN_SAMPLES)
            .into_iter()
            .map(|p| on_screen(to_world(p)))
            .collect();
        for stroke in [halo, line] {
            for w in pts.windows(2) {
                if let (Some(a), Some(b)) = (w[0], w[1]) {
                    painter.line_segment([a, b], stroke);
                }
            }
        }
        let chosen = *role == al.arch;
        for (i, p) in controls.iter().enumerate() {
            let Some(at) = on_screen(to_world(*p)) else {
                continue;
            };
            let r = if hot == Some((*role, i)) {
                POINT_RADIUS + 2.0
            } else {
                POINT_RADIUS
            };
            painter.circle_filled(at, r + 1.0, v.halo);
            if chosen {
                painter.circle_filled(at, r, v.color);
            } else {
                painter.circle_stroke(at, r - 1.0, Stroke::new(2.0, v.color));
            }
        }
    }
}

impl Workspace {
    /// The arch line is drawn: its tool is open and not hidden.
    pub(crate) fn arch_line_shown(&self) -> bool {
        self.manual.tool == Some(Tool::ArchLine) && !self.archline.hidden
    }

    /// The controls of the tool in the inspector of the Final position.
    pub(crate) fn arch_line_controls(&mut self, ui: &mut Ui) {
        let c = *colors(ui);
        let state = std::sync::Arc::clone(self.undo.current());
        self.archline.follow(&state);
        let with_line = arches_with_line(&state);
        let small = |ui: &mut Ui, key: &str| {
            ui.label(RichText::new(t(key)).color(c.text_secondary).small());
        };
        if with_line.is_empty() {
            small(ui, "ws-archline-none");
            ui.add_space(6.0);
            if Button::ghost(t("ws-manual-close")).show(ui).clicked() {
                self.close_tool();
            }
            return;
        }
        if !with_line.contains(&self.archline.arch) {
            self.archline.arch = with_line[0];
        }
        small(ui, "ws-archline-hint");
        ui.add_space(6.0);
        if with_line.len() > 1 {
            ui.horizontal(|ui| {
                for role in &with_line {
                    let key = if *role == ScanRole::Upper {
                        "ws-role-upper"
                    } else {
                        "ws-role-lower"
                    };
                    if Button::secondary(t(key))
                        .selected(self.archline.arch == *role)
                        .show(ui)
                        .clicked()
                    {
                        self.archline.arch = *role;
                    }
                }
            });
            ui.add_space(4.0);
        }
        let role = self.archline.arch;
        let drawn = line_of(&state, role).is_some_and(|(_, d)| d);
        let pending = self.archline.pending(&state, role);
        small(
            ui,
            if drawn {
                "ws-archline-is-drawn"
            } else {
                "ws-archline-is-auto"
            },
        );
        if pending {
            ui.label(
                RichText::new(t("ws-archline-pending"))
                    .color(c.status_warning)
                    .small(),
            );
        }
        ui.add_space(4.0);
        ui.checkbox(&mut self.archline.recompute, t("ws-archline-recompute"));
        let busy = self.is_busy();
        let mut apply = false;
        let mut reset = false;
        let mut close = false;
        ui.horizontal_wrapped(|ui| {
            apply = Button::primary(t("ws-archline-apply"))
                .enabled(!busy)
                .show(ui)
                .clicked();
            reset = Button::secondary(t("ws-archline-reset"))
                .enabled(!busy && (drawn || pending))
                .show(ui)
                .clicked();
        });
        ui.horizontal_wrapped(|ui| {
            let key = if self.archline.hidden {
                "ws-archline-show"
            } else {
                "ws-archline-hide"
            };
            if Button::secondary(t(key)).show(ui).clicked() {
                self.archline.hidden = !self.archline.hidden;
            }
            close = Button::ghost(t("ws-manual-close")).show(ui).clicked();
        });
        if apply {
            self.arch_line_apply();
        }
        if reset {
            self.arch_line_reset();
        }
        if close {
            self.close_tool();
        }
    }

    /// Apply: the line of the chosen arch as its points are now.
    pub(crate) fn arch_line_apply(&mut self) {
        let state = std::sync::Arc::clone(self.undo.current());
        if let Some(params) = self.archline.apply_params(&state, self.archline.arch) {
            self.run_manual(LINE, params);
        }
    }

    /// "Back to the automatic line": the points moved are forgotten and a
    /// drawn line is dropped (the final position computed again).
    pub(crate) fn arch_line_reset(&mut self) {
        let state = std::sync::Arc::clone(self.undo.current());
        let role = self.archline.arch;
        self.archline.edits.remove(&role);
        if line_of(&state, role).is_some_and(|(_, drawn)| drawn) {
            self.run_manual(LINE, reset_params(role));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(x: f64, y: f64, z: f64) -> DVec3 {
        DVec3::new(x, y, z)
    }

    #[test]
    fn the_curve_goes_through_its_points_with_the_plugin_sampling() {
        let c = [
            d(0.0, 0.0, 0.0),
            d(10.0, 5.0, 0.0),
            d(20.0, 0.0, 1.0),
            d(30.0, -5.0, 0.0),
        ];
        let p = curve(&c);
        assert_eq!(p.len(), (SAMPLES - 1) * (c.len() - 1) + 1);
        for (i, q) in c.iter().enumerate() {
            assert!(p[i * (SAMPLES - 1)].distance(*q) < 1e-12, "{i}");
        }
        // smooth: no corner at an inner point
        let k = SAMPLES - 1;
        let before = (p[k] - p[k - 1]).normalize();
        let after = (p[k + 1] - p[k]).normalize();
        assert!(before.dot(after) > 0.99);
        // points in a line stay in that line
        let straight = curve(&[d(0.0, 0.0, 0.0), d(1.0, 1.0, 0.0), d(3.0, 3.0, 0.0)]);
        assert!(
            straight
                .iter()
                .all(|q| (q.x - q.y).abs() < 1e-9 && q.z.abs() < 1e-12)
        );
    }

    #[test]
    fn resampling_spreads_the_points_evenly_along_the_line() {
        let line = [d(0.0, 0.0, 0.0), d(10.0, 0.0, 0.0), d(10.0, 10.0, 0.0)];
        let r = resample(&line, 5);
        let expect = [
            d(0.0, 0.0, 0.0),
            d(5.0, 0.0, 0.0),
            d(10.0, 0.0, 0.0),
            d(10.0, 5.0, 0.0),
            d(10.0, 10.0, 0.0),
        ];
        for (a, b) in r.iter().zip(expect) {
            assert!(a.distance(b) < 1e-12, "{a} vs {b}");
        }
    }

    #[test]
    fn a_dragged_point_stays_on_the_occlusal_plane() {
        let point = Vec3::new(1.0, 2.0, 3.0);
        // seen from above: on the horizontal plane through the point
        let w = dragged_to(
            point,
            Vec3::new(5.0, 5.0, 50.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::NEG_Z,
        )
        .unwrap();
        assert!((w - Vec3::new(5.0, 5.0, 3.0)).length() < 1e-5);
        let dir = Vec3::new(0.3, 0.0, -1.0).normalize();
        let w = dragged_to(point, Vec3::new(0.0, 0.0, 40.0), dir, dir).unwrap();
        assert!((w.z - 3.0).abs() < 1e-4);
        // edge-on: on the plane of the screen through the point
        let w = dragged_to(point, Vec3::new(0.0, -50.0, 3.5), Vec3::Y, Vec3::Y).unwrap();
        assert!((w - Vec3::new(0.0, 2.0, 3.5)).length() < 1e-5);
        // behind the camera: nothing
        assert!(dragged_to(point, Vec3::ZERO, Vec3::NEG_Z, Vec3::NEG_Z).is_none());
    }

    const WAIT: std::time::Duration = std::time::Duration::from_secs(120);

    fn exec(id: &str, state: &CaseState, params: Value) -> CaseState {
        dca_module::commands::find(id)
            .unwrap()
            .execute(state, &params, &dc_command::JobCtx::detached())
            .unwrap_or_else(|e| panic!("{id}: {e}"))
            .0
    }

    /// A lower demo arch with its tooth frames and final position.
    fn set_up() -> CaseState {
        let s = exec(
            "aligners.demo",
            &CaseState::default(),
            json!({ "arch": "lower", "severity": "moderate" }),
        );
        let s = exec("aligners.teeth.frames", &s, Value::Null);
        exec("aligners.planning.setup.automatic", &s, Value::Null)
    }

    fn workspace(state: CaseState) -> Workspace {
        let project = dc_project::Project::new(
            dc_project::AppInfo {
                version: "0.1.0".into(),
                commit: "test".into(),
            },
            crate::now_rfc3339(),
            state,
        );
        Workspace::new_headless(
            Box::new(dca_module::AlignersModule::new()),
            Some(project),
            None,
        )
    }

    fn setup(ws: &Workspace) -> dca_model::Setup {
        overlay::aligners(ws.undo.current()).unwrap().setup.clone()
    }

    fn ghosts(ws: &Workspace) -> usize {
        let g = crate::view::FINAL_GHOST;
        ws.view.meshes.iter().filter(|m| m.id & g == g).count()
    }

    /// Point `i` of the lower line moved 2 mm towards the incisors.
    fn move_forward(ws: &mut Workspace, i: usize) -> DVec3 {
        let st = std::sync::Arc::clone(ws.undo.current());
        let ahead = st
            .case
            .world_frame
            .from_case
            .inverse()
            .apply_vector(dc_case::WorldFrame::ANTERIOR);
        let c = ws.archline.controls(&st, ScanRole::Lower).unwrap();
        let to = c[i] + 2.0 * ahead;
        assert!(ws.archline.move_point(&st, ScanRole::Lower, i, to));
        to
    }

    #[test]
    fn the_tool_shows_the_line_with_the_final_position_and_runs_nothing() {
        let mut ws = workspace(set_up());
        let before = std::sync::Arc::clone(ws.undo.current());
        ws.sync_scene_with(dc_scene::Color::TOOTH);
        assert_eq!(ghosts(&ws), 0);
        assert!(ws.open_tool(Tool::ArchLine));
        assert_eq!(ws.selected(), Some(crate::leaf::SETUP));
        assert!(ws.arch_line_shown());
        assert!(ws.wait_for_jobs(WAIT));
        assert!(
            std::sync::Arc::ptr_eq(&before, ws.undo.current()),
            "nothing ran"
        );
        // the automatic line of the last computation, its points spread along it
        let (line, drawn) = line_of(&before, ScanRole::Lower).unwrap();
        assert!(!drawn);
        let c = ws.archline.controls(&before, ScanRole::Lower).unwrap();
        assert_eq!(c.len(), CONTROLS);
        assert!(c[0].distance(line[0]) < 1e-9);
        assert!(c[CONTROLS - 1].distance(line[line.len() - 1]) < 1e-9);
        assert!(line_of(&before, ScanRole::Upper).is_none());
        // the teeth at their final position are ghosted while it is shown
        ws.sync_scene_with(dc_scene::Color::TOOTH);
        assert!(ghosts(&ws) > 0);
        // Hide: no line and no ghost; Show brings them back
        ws.archline.hidden = true;
        assert!(!ws.arch_line_shown());
        ws.sync_scene_with(dc_scene::Color::TOOTH);
        assert_eq!(ghosts(&ws), 0);
        ws.archline.hidden = false;
        ws.sync_scene_with(dc_scene::Color::TOOTH);
        assert!(ghosts(&ws) > 0);
        // closing the tool takes them away
        ws.close_tool();
        assert!(!ws.arch_line_shown());
        ws.sync_scene_with(dc_scene::Color::TOOTH);
        assert_eq!(ghosts(&ws), 0);
    }

    #[test]
    fn the_moved_line_is_applied_in_one_undo_step_and_back_to_automatic() {
        let mut ws = workspace(set_up());
        let auto = setup(&ws);
        assert!(ws.open_tool(Tool::ArchLine));
        let to = move_forward(&mut ws, 5);
        let st = std::sync::Arc::clone(ws.undo.current());
        assert!(ws.archline.pending(&st, ScanRole::Lower));
        assert!(
            std::sync::Arc::ptr_eq(&st, ws.undo.current()),
            "kept in the tool"
        );
        ws.arch_line_apply();
        assert!(ws.wait_for_jobs(WAIT));
        ws.manual_feedback();
        assert!(ws.toast_keys().iter().any(|k| k == "ws-archline-done"));
        let drawn = setup(&ws);
        let line = &drawn.drawn[&ScanRole::Lower];
        assert_eq!(line.len(), (SAMPLES - 1) * (CONTROLS - 1) + 1);
        // through the point moved
        assert!(line[5 * (SAMPLES - 1)].distance(to) < 1e-3);
        // the final position computed again on it
        assert_ne!(drawn.poses, auto.poses);
        assert_eq!(
            line_of(ws.undo.current(), ScanRole::Lower).map(|(_, d)| d),
            Some(true)
        );
        // the moved points are forgotten: the tool shows the stored line
        let now = std::sync::Arc::clone(ws.undo.current());
        ws.archline.follow(&now);
        assert!(!ws.archline.pending(&now, ScanRole::Lower));
        // one undo step
        assert!(ws.undo());
        assert_eq!(setup(&ws), auto);
        assert!(ws.redo());
        assert_eq!(setup(&ws), drawn);
        // back to the automatic line: the final position as before
        ws.arch_line_reset();
        assert!(ws.wait_for_jobs(WAIT));
        let back = setup(&ws);
        assert!(back.drawn.is_empty());
        assert_eq!(back.poses, auto.poses);
        assert_eq!(back.lines, auto.lines);
    }

    #[test]
    fn apply_without_recompute_only_keeps_the_line_and_back_forgets_moved_points() {
        let mut ws = workspace(set_up());
        let auto = setup(&ws);
        assert!(ws.open_tool(Tool::ArchLine));
        move_forward(&mut ws, 3);
        ws.archline.recompute = false;
        ws.arch_line_apply();
        assert!(ws.wait_for_jobs(WAIT));
        let kept = setup(&ws);
        assert_eq!(kept.poses, auto.poses);
        assert_eq!(kept.lines, auto.lines);
        assert!(kept.drawn.contains_key(&ScanRole::Lower));
        // moved points not applied: "back" forgets them and runs nothing
        assert!(ws.undo());
        move_forward(&mut ws, 2);
        let st = std::sync::Arc::clone(ws.undo.current());
        ws.arch_line_reset();
        assert!(ws.wait_for_jobs(WAIT));
        assert!(std::sync::Arc::ptr_eq(&st, ws.undo.current()));
        assert!(!ws.archline.pending(&st, ScanRole::Lower));
    }
}
