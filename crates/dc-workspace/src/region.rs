//! The view of the region of a tooth on the scan ("Show the region" in the
//! inspector of the 3D teeth; `oa.tooth_highlight` and `oa.tooth_highlight_end`
//! of the plugin).
//!
//! It shows from which part of the scan the selected tooth is built: the scan
//! is drawn instead of the 3D teeth, at the initial position, the tooth keeps
//! its colour and the other colours are dimmed ([`DIM`] of the way to grey, as
//! in the plugin), and the margin line, the crown / gingiva boundary the closed
//! tooth is cut along, is drawn in yellow (the `model_margin` token) over the
//! mesh, in front of it, so it is seen from any angle. The tooth is the one
//! selected in the viewport (a click on a tooth or on its number); another
//! click moves the view to that tooth.
//!
//! The view only reads the case. The margin line and the check of the region
//! come from the read-only command `aligners.manual.tooth-region`, the same
//! one the CLI and the MCP server run, executed on the current state without
//! touching the undo stack; nothing is run by the menu law's meaning (no
//! step, no change, no dialog). The view ends with "End", Escape, a selection
//! that is no longer a tooth, a tool opened, or another function selected;
//! the model (scan or 3D teeth) and the layers that were shown come back,
//! because the view only overrides them while it lasts.

use std::sync::Arc;

use dc_case::CaseState;
use dc_command::JobCtx;
use dc_scene::{Color, LabelPalette};
use dc_ui::icons as ic;
use dc_ui::theme::colors;
use dc_ui::widgets::Button;
use dc_viewport::Viewport;
use egui::{Color32, Rect, Stroke, Ui};
use glam::{Mat4, Vec3};
use serde_json::{Value, json};

use crate::function::caption;
use crate::select::Component;
use crate::workspace::Workspace;
use crate::{manual, t};

/// The read-only query of the region of a tooth.
pub(crate) const REGION: &str = "aligners.manual.tooth-region";

/// Colours outside the region are blended this much towards grey
/// (`HIGHLIGHT_DIM` of the plugin).
pub(crate) const DIM: f32 = 0.65;
/// The grey the other colours are blended towards.
const GREY: f32 = 0.55;
/// Width of the margin line on screen (points).
const MARGIN_WIDTH: f32 = 2.5;

/// What the view of the region keeps.
#[derive(Default)]
pub(crate) struct RegionView {
    /// The view is on (it also needs a selected tooth: [`Workspace::region_tooth`]).
    pub on: bool,
    /// What the query answered for the case state and the tooth it was asked for.
    data: Option<Arc<RegionData>>,
}

/// The answer of the query for one tooth of one case state.
pub(crate) struct RegionData {
    state: Arc<CaseState>,
    pub fdi: u8,
    /// The margin line as chains of points in the coordinates of the scan
    /// mesh, each with whether it closes on itself.
    pub lines: Vec<(Vec<Vec3>, bool)>,
    /// Length of the margin line (mm).
    pub length_mm: f64,
    /// Points of the region.
    pub vertices: u64,
    /// Area, width of the region and the reasons of the check, when the arch
    /// could be measured.
    pub check: Option<Check>,
    /// Why the query failed (the arch is not separated into teeth yet...).
    pub error: Option<String>,
}

/// The check of the region (`dca_algo::manual::RegionReport`).
pub(crate) struct Check {
    pub area_mm2: f64,
    pub width_mm: f64,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl RegionData {
    /// The answer in `out`, the JSON of the command.
    fn from_output(state: Arc<CaseState>, fdi: u8, out: &Value) -> Self {
        let num = |v: &Value| v.as_f64().unwrap_or_default();
        let lines = out["margin"]["polylines"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|l| {
                        let pts = l["points"].as_array().map_or_else(Vec::new, |p| {
                            p.iter()
                                .map(|q| {
                                    Vec3::new(
                                        num(&q[0]) as f32,
                                        num(&q[1]) as f32,
                                        num(&q[2]) as f32,
                                    )
                                })
                                .collect()
                        });
                        (pts, l["closed"].as_bool().unwrap_or(false))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let check = out["region"].is_object().then(|| Check {
            area_mm2: num(&out["region"]["area_mm2"]),
            width_mm: num(&out["region"]["width_mm"]),
            errors: strings(&out["region"]["errors"]),
            warnings: strings(&out["region"]["warnings"]),
        });
        Self {
            state,
            fdi,
            lines,
            length_mm: num(&out["margin"]["length_mm"]),
            vertices: out["vertices"].as_u64().unwrap_or_default(),
            check,
            error: None,
        }
    }

    fn failed(state: Arc<CaseState>, fdi: u8, why: String) -> Self {
        Self {
            state,
            fdi,
            lines: Vec::new(),
            length_mm: 0.0,
            vertices: 0,
            check: None,
            error: Some(why),
        }
    }
}

/// A colour blended towards grey (the colours outside the region).
fn dim(c: Color) -> Color {
    let mix = |x: f32| x * (1.0 - DIM) + GREY * DIM;
    Color {
        r: mix(c.r),
        g: mix(c.g),
        b: mix(c.b),
        a: c.a,
    }
}

/// The colour of every vertex of a labelled scan in the view of the region:
/// the vertices labelled `fdi` in their colour, all the others dimmed (every
/// vertex dimmed when `fdi` is `None`, the scan of the other arch).
pub(crate) fn region_colors(labels: &[u16], palette: &LabelPalette, fdi: Option<u8>) -> Vec<Color> {
    labels
        .iter()
        .map(|&l| {
            let c = palette.color(l);
            if fdi.is_some_and(|f| u16::from(f) == l) {
                c
            } else {
                dim(c)
            }
        })
        .collect()
}

/// Draws the margin line over the viewport (in front of the mesh, from any
/// angle). `xform` takes the scan mesh to the world, as the viewport mesh has
/// it.
pub(crate) fn paint_margin(
    ui: &Ui,
    vp: &Viewport,
    rect: Rect,
    data: &RegionData,
    xform: Mat4,
    color: Color32,
) {
    let painter = ui.painter_at(rect);
    let cam = vp.camera();
    let eye = cam.eye();
    let fwd = (cam.target - eye).normalize_or_zero();
    let halo = Stroke::new(MARGIN_WIDTH + 2.0, Color32::from_black_alpha(120));
    let line = Stroke::new(MARGIN_WIDTH, color);
    // in front of the camera: a point behind it has no place on the screen
    let at = |p: Vec3| {
        let w = xform.transform_point3(p);
        (fwd.dot(w - eye) > 0.0).then(|| vp.screen_pos(rect, w))
    };
    for stroke in [halo, line] {
        for (points, closed) in &data.lines {
            let n = points.len();
            let segments = if *closed { n } else { n.saturating_sub(1) };
            for k in 0..segments {
                if let (Some(a), Some(b)) = (at(points[k]), at(points[(k + 1) % n])) {
                    painter.line_segment([a, b], stroke);
                }
            }
        }
    }
}

impl Workspace {
    /// The tooth whose region is shown: the view is on and a tooth is selected.
    pub(crate) fn region_tooth(&self) -> Option<u8> {
        if !self.region.on {
            return None;
        }
        match self.selection.component {
            Some(Component::Tooth(f)) => Some(f),
            _ => None,
        }
    }

    /// The module has the query the view reads.
    fn region_ready(&self) -> bool {
        self.specs.iter().any(|s| s.id == REGION)
    }

    /// Turns the view on or off. It never changes the case, and it ends any
    /// open tool (a tool works on the scan with its own colours).
    pub(crate) fn set_region_view(&mut self, on: bool) {
        if on {
            self.close_tool();
        }
        self.region.on = on;
        if !on {
            self.region.data = None;
        }
    }

    /// Escape: ends the view. `false` when it was not on.
    pub(crate) fn region_escape(&mut self) -> bool {
        let was = self.region.on;
        self.set_region_view(false);
        was
    }

    /// The view follows the selection: it ends when what is selected is not a
    /// tooth any more (deselected, or another component).
    pub(crate) fn region_follow(&mut self) {
        if self.region.on && !matches!(self.selection.component, Some(Component::Tooth(_))) {
            self.set_region_view(false);
        }
    }

    /// What the query says of the selected tooth, asked once per case state
    /// and tooth (not on every frame).
    pub(crate) fn region_data(&mut self) -> Option<Arc<RegionData>> {
        let fdi = self.region_tooth()?;
        let state = Arc::clone(self.undo.current());
        if let Some(d) = &self.region.data
            && d.fdi == fdi
            && Arc::ptr_eq(&d.state, &state)
        {
            return Some(Arc::clone(d));
        }
        let spec = self.specs.iter().find(|s| s.id == REGION)?;
        let data = match spec.execute(
            &state,
            &json!({ "fdi": fdi, "detail": true }),
            &JobCtx::detached(),
        ) {
            Ok((_, out)) => RegionData::from_output(state, fdi, &out),
            Err(e) => RegionData::failed(state, fdi, e.to_string()),
        };
        let data = Arc::new(data);
        self.region.data = Some(Arc::clone(&data));
        Some(data)
    }

    /// What the viewport needs to draw the margin line: the answer and the
    /// transform of the scan mesh of the tooth's arch.
    pub(crate) fn region_overlay(&mut self) -> Option<(Arc<RegionData>, Mat4)> {
        let data = self.region_data()?;
        let role = if crate::labels::upper_tooth(data.fdi) {
            dc_case::ScanRole::Upper
        } else {
            dc_case::ScanRole::Lower
        };
        let id = manual::scan_mesh_id(data.state.case.scan(role)?);
        let xform = self
            .view
            .meshes
            .iter()
            .find(|m| m.id == id && m.visible)?
            .xform;
        Some((data, xform))
    }

    /// The controls of the view in the inspector of the 3D teeth: "Show the
    /// region" for the selected tooth and "End", and what the region is.
    pub(crate) fn region_controls(&mut self, ui: &mut Ui) {
        let col = *colors(ui);
        let selected = match self.selection.component {
            Some(Component::Tooth(f)) => Some(f),
            _ => None,
        };
        let on = self.region_tooth().is_some();
        let ready = self.region_ready();
        ui.columns(2, |cols| {
            let show = Button::secondary(t("ws-region-show"))
                .icon(ic::EYE)
                .selected(on)
                .enabled(ready && selected.is_some() && !on)
                .full_width(true)
                .show(&mut cols[0]);
            let end = Button::secondary(t("ws-region-end"))
                .icon(ic::EYE_SLASH)
                .enabled(on)
                .full_width(true)
                .show(&mut cols[1]);
            if show.clicked() {
                self.set_region_view(true);
            }
            if end.clicked() {
                self.set_region_view(false);
            }
        });
        ui.add_space(4.0);
        if !ready {
            caption(ui, t("ws-region-unavailable"));
            return;
        }
        let Some(fdi) = selected else {
            caption(ui, t("ws-region-pick"));
            return;
        };
        let fdi_text = fdi.to_string();
        if !on {
            caption(ui, t("ws-region-hint"));
            return;
        }
        caption(ui, dc_i18n::tr_args("ws-region-on", &[("fdi", &fdi_text)]));
        let Some(data) = self.region_data() else {
            return;
        };
        if let Some(why) = &data.error {
            caption(ui, why.clone());
            return;
        }
        if data.vertices == 0 {
            caption(
                ui,
                dc_i18n::tr_args("ws-region-no-region", &[("fdi", &fdi_text)]),
            );
            return;
        }
        if let Some(c) = &data.check {
            let stats = dc_i18n::tr_args(
                "ws-region-stats",
                &[
                    ("vertices", &data.vertices.to_string()),
                    ("area", &format!("{:.0}", c.area_mm2)),
                    ("width", &format!("{:.1}", c.width_mm)),
                ],
            );
            caption(ui, stats);
            for e in &c.errors {
                ui.label(egui::RichText::new(e).color(col.status_danger));
            }
            for w in &c.warnings {
                ui.label(egui::RichText::new(w).color(col.status_warning));
            }
        }
        if data.lines.is_empty() {
            caption(ui, t("ws-region-no-margin"));
        } else {
            caption(
                ui,
                dc_i18n::tr_args(
                    "ws-region-margin",
                    &[("length", &format!("{:.1}", data.length_mm))],
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> LabelPalette {
        LabelPalette::default()
    }

    #[test]
    fn the_region_keeps_its_colour_and_the_rest_is_dimmed() {
        let p = palette();
        let labels = [0u16, 36, 35, 36];
        let c = region_colors(&labels, &p, Some(36));
        assert_eq!(c[1], p.color(36));
        assert_eq!(c[3], p.color(36));
        for k in [0usize, 2] {
            let (own, dimmed) = (p.color(labels[k]), c[k]);
            assert_ne!(dimmed, own);
            // blended towards grey: between the colour and the grey
            for (a, b) in [(own.r, dimmed.r), (own.g, dimmed.g), (own.b, dimmed.b)] {
                let expected = a * (1.0 - DIM) + GREY * DIM;
                assert!((b - expected).abs() < 1e-6);
            }
        }
        // no tooth: the other arch, all dimmed
        let all = region_colors(&labels, &p, None);
        assert!(all.iter().zip(&labels).all(|(c, &l)| *c != p.color(l)));
    }

    #[test]
    fn the_answer_of_the_query_is_read_with_its_margin_and_check() {
        let out = json!({
            "fdi": 36, "vertices": 120, "faces": 200,
            "region": { "errors": [], "warnings": ["w"], "area_mm2": 91.0, "width_mm": 11.0 },
            "margin": {
                "length_mm": 30.5,
                "polylines": [
                    { "closed": true, "points": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]] },
                    { "closed": false, "points": [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0]] },
                ],
            },
        });
        let d = RegionData::from_output(Arc::new(CaseState::default()), 36, &out);
        assert_eq!((d.fdi, d.vertices), (36, 120));
        assert_eq!(d.lines.len(), 2);
        assert_eq!((d.lines[0].0.len(), d.lines[0].1), (3, true));
        assert_eq!((d.lines[1].0.len(), d.lines[1].1), (2, false));
        assert_eq!(d.lines[0].0[2], Vec3::new(1.0, 1.0, 0.0));
        let c = d.check.unwrap();
        assert_eq!((c.area_mm2, c.width_mm), (91.0, 11.0));
        assert_eq!(c.warnings, ["w"]);
        assert!((d.length_mm - 30.5).abs() < 1e-12 && d.error.is_none());
        // a refused query
        let f = RegionData::failed(Arc::new(CaseState::default()), 36, "no".into());
        assert!(f.lines.is_empty() && f.error.as_deref() == Some("no"));
    }
}
