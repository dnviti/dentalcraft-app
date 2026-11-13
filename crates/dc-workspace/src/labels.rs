//! The tooth numbers drawn over the 3D viewport (a chip with the FDI number
//! in the colour of the tooth, as the plugin draws them) and the hit test of
//! the Alt+click that chooses the brush tooth.
//!
//! Everything here is a pure function of numbers (anchors, screen positions,
//! rectangles), so it runs without a GPU; `view.rs` projects the anchors with
//! the viewport and calls them.

use std::collections::BTreeMap;

use dc_case::{CaseState, Fdi, ScanRole};
use dca_model::Stage;
use egui::{Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use glam::{DVec3, Mat4, Vec3};

use crate::manual::Action;
use crate::overlay;

/// Chip size (points), as the plugin's 28 x 17 px chip.
pub(crate) const CHIP_SIZE: Vec2 = vec2(28.0, 17.0);
/// Text size of the number.
pub(crate) const TEXT_SIZE: f32 = 11.0;
/// Corner radius of the chip.
const CHIP_RADIUS: u8 = 4;
/// How far above its anchor the chip sits on the screen (points).
pub(crate) const LIFT: f32 = 6.0;
/// Space kept between two chips (points).
const CHIP_GAP: f32 = 2.0;
/// Most steps a chip is moved up to leave another one.
const MAX_NUDGES: usize = 4;
/// A hit on a gem counts this much outside its drawn radius (points).
pub(crate) const GEM_SLACK_PX: f32 = 4.0;
/// A point is hidden when the scan is this much nearer to the viewer than
/// it along the line of sight (mm).
pub(crate) const OCCLUSION_TOLERANCE_MM: f32 = 1.0;

/// Where the number of a tooth is anchored, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum AnchorKind {
    /// The cusp tip measured by the teeth stage.
    Tip,
    /// The top of the crown painted on the scan.
    Crown,
    /// The tooth mark.
    Seed,
}

/// The point of the scene a tooth number belongs to (world coordinates, mm,
/// teeth at their initial position).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Anchor {
    pub fdi: u8,
    /// The tooth is in the upper arch.
    pub upper: bool,
    pub kind: AnchorKind,
    pub world: Vec3,
}

/// The tooth (FDI number) is in the upper arch: quadrants 1 and 2, 5 and 6
/// for the deciduous teeth.
pub(crate) fn upper_tooth(fdi: u8) -> bool {
    matches!(fdi / 10, 1 | 2 | 5 | 6)
}

/// The anchor of a tooth: its tip when known, else the top of its crown,
/// else its seed.
pub(crate) fn choose_anchor(
    tip: Option<Vec3>,
    crown: Option<Vec3>,
    seed: Option<Vec3>,
) -> Option<(AnchorKind, Vec3)> {
    tip.map(|p| (AnchorKind::Tip, p))
        .or(crown.map(|p| (AnchorKind::Crown, p)))
        .or(seed.map(|p| (AnchorKind::Seed, p)))
}

/// The top of every labelled crown of a scan: the centre of its vertices
/// raised to the highest vertex along `up` (as the plugin does). `positions`
/// are in the coordinates of the scan; `xform` takes them to the world.
pub(crate) fn crown_spots(
    positions: &[Vec3],
    labels: &[u16],
    xform: Mat4,
    up: Vec3,
) -> BTreeMap<u8, Vec3> {
    // label -> (sum, count, highest)
    let mut acc: BTreeMap<u16, (Vec3, u32, f32)> = BTreeMap::new();
    for (p, &l) in positions.iter().zip(labels) {
        if l == 0 {
            continue;
        }
        let w = xform.transform_point3(*p);
        let e = acc.entry(l).or_insert((Vec3::ZERO, 0, f32::NEG_INFINITY));
        e.0 += w;
        e.1 += 1;
        e.2 = e.2.max(w.dot(up));
    }
    acc.into_iter()
        .filter_map(|(l, (sum, n, top))| {
            let fdi = u8::try_from(l).ok()?;
            let c = sum / n as f32;
            Some((fdi, c + up * (top - c.dot(up))))
        })
        .collect()
}

/// The anchors of the teeth of the case, in world coordinates, sorted by
/// tooth number, with the crown tops of the built teeth. A tooth marked
/// missing has none.
#[cfg(test)]
pub(crate) fn anchors(state: &CaseState) -> Vec<Anchor> {
    anchors_of(state, true)
}

/// [`anchors`] for what the viewport draws: with `built` the crown top of a
/// built tooth is the top of its closed mesh (the 3D teeth are drawn), else
/// the top of its crown painted on the scan (the scan is drawn). An arch
/// whose built teeth are out of date shows its scan whatever `built` says
/// ([`overlay::outdated_arches`]).
pub(crate) fn anchors_of(state: &CaseState, built: bool) -> Vec<Anchor> {
    let Some(al) = overlay::aligners(state) else {
        return Vec::new();
    };
    let world = state.case.world_frame.from_case;
    let wm = world.to_mat4().as_mat4();
    let to_world = |p: DVec3| world.apply_point(p).as_vec3();
    let mut crowns: BTreeMap<u8, Vec3> = BTreeMap::new();
    for scan in &state.case.scans {
        if scan.role == ScanRole::Bite {
            continue;
        }
        let Some(mesh) = state.mesh(&scan.mesh) else {
            continue;
        };
        let Some(labels) =
            overlay::scan_labels(state, scan.role, mesh).and_then(|m| m.vertex_labels.as_ref())
        else {
            continue;
        };
        // the occlusal plane is XY; the upper teeth point down, the lower up
        let up = if scan.role == ScanRole::Upper {
            Vec3::NEG_Z
        } else {
            Vec3::Z
        };
        let xform = wm * scan.import_xform.to_mat4().as_mat4();
        crowns.extend(crown_spots(&mesh.positions, labels, xform, up));
    }
    // a built tooth: the top of its closed mesh (drawn instead of the scan)
    let outdated = overlay::outdated_arches(state);
    let drawn = |f: &Fdi| {
        let role = if f.is_upper() {
            ScanRole::Upper
        } else {
            ScanRole::Lower
        };
        built && !outdated.contains(&role)
    };
    for (f, blob) in al.crowns.iter().filter(|(f, _)| drawn(f)) {
        let Some(mesh) = state.mesh(blob) else {
            continue;
        };
        let up = if f.is_upper() { Vec3::NEG_Z } else { Vec3::Z };
        let l = vec![u16::from(f.get()); mesh.vertex_count()];
        crowns.extend(crown_spots(&mesh.positions, &l, wm, up));
    }
    let mut fdis: Vec<u8> = crowns
        .keys()
        .copied()
        .chain(al.seeds.iter().map(|s| s.fdi.get()))
        .chain(al.tooth_geometry.keys().map(|f| f.get()))
        .collect();
    fdis.sort_unstable();
    fdis.dedup();
    fdis.into_iter()
        .filter(|f| !al.missing.iter().any(|m| m.get() == *f))
        .filter_map(|fdi| {
            let tip = al
                .tooth_geometry
                .iter()
                .find(|(f, _)| f.get() == fdi)
                .map(|(_, m)| to_world(m.tip));
            let seed = al
                .seeds
                .iter()
                .find(|s| s.fdi.get() == fdi)
                .map(|s| to_world(s.position));
            let (kind, world) = choose_anchor(tip, crowns.get(&fdi).copied(), seed)?;
            Some(Anchor {
                fdi,
                upper: upper_tooth(fdi),
                kind,
                world,
            })
        })
        .collect()
}

/// `p`, a point of the tooth `fdi` in the world, moved to where the tooth is
/// at `stage` (the pose is relative to the rest frame of the tooth, as in
/// `overlay::posed_mesh`).
pub(crate) fn posed_point(state: &CaseState, stage: &Stage, fdi: u8, p: Vec3) -> Vec3 {
    let Some(al) = overlay::aligners(state) else {
        return p;
    };
    let Some((f, pose)) = stage.poses.iter().find(|(f, _)| f.get() == fdi) else {
        return p;
    };
    let Some(fr) = al.tooth_frames.get(f) else {
        return p;
    };
    let world = state.case.world_frame.from_case;
    let moved = fr.pose.compose(pose).compose(&fr.pose.inverse());
    // case coordinates -> moved -> world
    let case_p = world.inverse().apply_point(p.as_dvec3());
    world.apply_point(moved.apply_point(case_p)).as_vec3()
}

/// The scan hit at `hit` (the first thing the line of sight from the camera
/// meets, looking along `fwd`) covers `anchor`: it is nearer to the viewer
/// by more than `tolerance` mm. Without a hit nothing covers it.
pub(crate) fn covers(fwd: Vec3, hit: Option<Vec3>, anchor: Vec3, tolerance: f32) -> bool {
    hit.is_some_and(|h| fwd.dot(h - anchor) < -tolerance)
}

/// Colour of the chip of a tooth (the palette of the scans).
pub(crate) fn chip_fill(fdi: u8) -> Color32 {
    crate::manual::pos_color(overlay::palette().color(u16::from(fdi)))
}

/// Linear value of an sRGB channel.
fn linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Contrast ratio between two colours (WCAG).
pub(crate) fn contrast(a: Color32, b: Color32) -> f32 {
    let lum = |c: Color32| 0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b());
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Black or white, whichever reads better on `fill`.
pub(crate) fn text_color(fill: Color32) -> Color32 {
    if contrast(fill, Color32::BLACK) >= contrast(fill, Color32::WHITE) {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// The rectangles of the chips, in the order given (the first wins its
/// place). A chip sits centred above its anchor, `lift` points up; one that
/// would cover a chip placed before is moved up, a few steps at most. Chips
/// whose anchor is outside `bounds` are dropped.
pub(crate) fn place_chips(
    anchors: &[(u8, Pos2)],
    size: Vec2,
    lift: f32,
    bounds: Rect,
) -> Vec<(u8, Rect)> {
    let mut placed: Vec<(u8, Rect)> = Vec::new();
    for &(fdi, at) in anchors {
        if !bounds.contains(at) {
            continue;
        }
        let mut rect = Rect::from_min_size(pos2(at.x - size.x / 2.0, at.y - lift - size.y), size);
        for _ in 0..MAX_NUDGES {
            let Some((_, hit)) = placed
                .iter()
                .find(|(_, r)| r.expand(CHIP_GAP / 2.0).intersects(rect))
            else {
                break;
            };
            rect = rect.translate(vec2(0.0, hit.top() - CHIP_GAP - rect.bottom()));
        }
        placed.push((fdi, rect));
    }
    placed
}

/// A gem on the screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Gem {
    pub fdi: u8,
    pub center: Pos2,
    /// Radius of the gem as drawn (points).
    pub radius: f32,
}

/// What an Alt+click lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hit {
    /// The number chip of a tooth.
    Label(u8),
    /// The gem of a tooth.
    Gem(u8),
    /// The scan, at a vertex with this label (`0` = gingiva).
    Surface(u8),
    /// Nothing.
    Nothing,
}

/// What is under `pos`, nearest to the viewer first: a number chip (the last
/// drawn is on top), then a gem (the nearest to the pointer within its
/// radius and a few points), then the scan.
pub(crate) fn hit_test(pos: Pos2, chips: &[(u8, Rect)], gems: &[Gem], surface: Option<u16>) -> Hit {
    if let Some((f, _)) = chips.iter().rev().find(|(_, r)| r.contains(pos)) {
        return Hit::Label(*f);
    }
    let gem = gems
        .iter()
        .map(|g| (g.center.distance(pos), g))
        .filter(|(d, g)| *d <= g.radius + GEM_SLACK_PX)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, g)) = gem {
        return Hit::Gem(g.fdi);
    }
    match surface {
        Some(l) => Hit::Surface(u8::try_from(l).unwrap_or(0)),
        None => Hit::Nothing,
    }
}

/// What an Alt+click on `hit` does: chooses the brush tooth (the gingiva
/// for the bare gingiva of the scan).
pub(crate) fn alt_click_action(hit: Hit) -> Action {
    match hit {
        Hit::Label(f) | Hit::Gem(f) | Hit::Surface(f) => Action::SelectBrush(f),
        Hit::Nothing => Action::None,
    }
}

/// What the chips drawn this frame cover, for the hit test.
#[derive(Debug, Clone, Default)]
pub(crate) struct Drawn {
    pub chips: Vec<(u8, Rect)>,
    pub gems: Vec<Gem>,
}

/// Draws the chips. `brush` is the tooth the brush works on (outlined), and
/// `hot` the one under the pointer while Alt is held.
pub(crate) fn paint(
    ui: &Ui,
    clip: Rect,
    chips: &[(u8, Rect)],
    outline: Color32,
    accent: Color32,
    brush: Option<u8>,
    hot: Option<u8>,
) {
    let painter = ui.painter_at(clip);
    for (fdi, rect) in chips {
        let fill = chip_fill(*fdi);
        let radius = CornerRadius::same(CHIP_RADIUS);
        painter.rect_filled(*rect, radius, fill);
        let stroke = if Some(*fdi) == hot || Some(*fdi) == brush {
            Stroke::new(2.0, accent)
        } else {
            Stroke::new(1.0, outline)
        };
        painter.rect_stroke(*rect, radius, stroke, StrokeKind::Inside);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            fdi.to_string(),
            dc_ui::fonts::mono(TEXT_SIZE),
            text_color(fill),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))
    }

    #[test]
    fn the_text_is_black_or_white_by_contrast() {
        assert_eq!(text_color(Color32::from_rgb(250, 230, 60)), Color32::BLACK);
        assert_eq!(text_color(Color32::from_rgb(20, 40, 160)), Color32::WHITE);
        assert_eq!(text_color(Color32::WHITE), Color32::BLACK);
        assert_eq!(text_color(Color32::BLACK), Color32::WHITE);
        // every colour of the palette reads (4.5:1 is the AA ratio for small text)
        for fdi in (11..=18).chain(21..=28).chain(31..=38).chain(41..=48) {
            let fill = chip_fill(fdi);
            let ratio = contrast(fill, text_color(fill));
            assert!(ratio >= 4.5, "tooth {fdi}: {ratio}");
        }
    }

    #[test]
    fn a_number_behind_the_scan_is_hidden_and_one_on_its_surface_is_not() {
        let fwd = Vec3::NEG_Z;
        let anchor = Vec3::new(0.0, 0.0, 0.0);
        // the scan is hit 5 mm nearer to the camera (higher z): hidden
        assert!(covers(
            fwd,
            Some(Vec3::new(0.0, 0.0, 5.0)),
            anchor,
            OCCLUSION_TOLERANCE_MM
        ));
        // the anchor is on the surface (the hit is within the tolerance)
        assert!(!covers(
            fwd,
            Some(Vec3::new(0.0, 0.0, 0.4)),
            anchor,
            OCCLUSION_TOLERANCE_MM
        ));
        // the hit is behind the anchor, or there is none
        assert!(!covers(
            fwd,
            Some(Vec3::new(0.0, 0.0, -3.0)),
            anchor,
            OCCLUSION_TOLERANCE_MM
        ));
        assert!(!covers(fwd, None, anchor, OCCLUSION_TOLERANCE_MM));
    }

    #[test]
    fn the_chip_is_in_the_colour_of_the_tooth() {
        let c = overlay::palette().color(36);
        assert_eq!(chip_fill(36), crate::manual::pos_color(c));
        assert_ne!(chip_fill(36), chip_fill(37));
    }

    #[test]
    fn the_anchor_is_the_tip_then_the_crown_top_then_the_seed() {
        let (t, c, s) = (Vec3::X, Vec3::Y, Vec3::Z);
        assert_eq!(
            choose_anchor(Some(t), Some(c), Some(s)),
            Some((AnchorKind::Tip, t))
        );
        assert_eq!(
            choose_anchor(None, Some(c), Some(s)),
            Some((AnchorKind::Crown, c))
        );
        assert_eq!(
            choose_anchor(None, None, Some(s)),
            Some((AnchorKind::Seed, s))
        );
        assert_eq!(choose_anchor(None, None, None), None);
        assert!(AnchorKind::Tip < AnchorKind::Crown && AnchorKind::Crown < AnchorKind::Seed);
    }

    #[test]
    fn the_crown_top_is_the_centre_raised_to_the_highest_vertex() {
        // tooth 11: four vertices of a crown, the highest at z = 5 (the
        // upper teeth point down: "up" is -Z, so the top is the lowest z)
        let pos = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 2.0, -5.0),
            Vec3::new(2.0, 2.0, 0.0),
            Vec3::new(9.0, 9.0, 9.0),
        ];
        let labels = [11, 11, 11, 11, 0];
        let spots = crown_spots(&pos, &labels, Mat4::IDENTITY, Vec3::NEG_Z);
        assert_eq!(spots.len(), 1);
        let s = spots[&11];
        assert!((s - Vec3::new(1.0, 1.0, -5.0)).length() < 1e-5, "{s:?}");
        // the transform is applied before the centre is taken
        let moved = crown_spots(
            &pos,
            &labels,
            Mat4::from_translation(Vec3::X * 10.0),
            Vec3::NEG_Z,
        );
        assert!((moved[&11] - Vec3::new(11.0, 1.0, -5.0)).length() < 1e-5);
    }

    #[test]
    fn a_chip_sits_above_its_anchor_and_off_screen_ones_are_dropped() {
        let placed = place_chips(
            &[(11, pos2(100.0, 100.0)), (12, pos2(-5.0, 10.0))],
            CHIP_SIZE,
            LIFT,
            bounds(),
        );
        assert_eq!(placed.len(), 1);
        let (f, r) = placed[0];
        assert_eq!(f, 11);
        assert_eq!(r.center().x, 100.0);
        assert_eq!(r.bottom(), 100.0 - LIFT);
        assert_eq!(r.size(), CHIP_SIZE);
    }

    #[test]
    fn chips_that_would_overlap_are_moved_up() {
        let a = [
            (11, pos2(100.0, 100.0)),
            (21, pos2(104.0, 102.0)),
            (12, pos2(300.0, 100.0)),
        ];
        let placed = place_chips(&a, CHIP_SIZE, LIFT, bounds());
        assert_eq!(placed.len(), 3);
        assert!(!placed[0].1.intersects(placed[1].1));
        // the first one stays, the second goes above it
        assert!(placed[1].1.bottom() <= placed[0].1.top());
        // the far one is where it would be alone
        assert_eq!(placed[2].1.bottom(), 100.0 - LIFT);
    }

    fn drawn() -> (Vec<(u8, Rect)>, Vec<Gem>) {
        let chips = vec![
            (11, Rect::from_min_size(pos2(100.0, 100.0), CHIP_SIZE)),
            // drawn later, so on top of 11
            (21, Rect::from_min_size(pos2(110.0, 105.0), CHIP_SIZE)),
        ];
        let gems = vec![
            Gem {
                fdi: 36,
                center: pos2(112.0, 108.0),
                radius: 6.0,
            },
            Gem {
                fdi: 37,
                center: pos2(300.0, 300.0),
                radius: 6.0,
            },
        ];
        (chips, gems)
    }

    #[test]
    fn the_hit_goes_label_then_gem_then_surface() {
        let (chips, gems) = drawn();
        // on a chip, over a gem and over the scan: the chip
        assert_eq!(
            hit_test(pos2(112.0, 108.0), &chips, &gems, Some(36)),
            Hit::Label(21)
        );
        // on the part of 11 that 21 does not cover: 11
        assert_eq!(
            hit_test(pos2(102.0, 102.0), &chips, &gems, Some(36)),
            Hit::Label(11)
        );
        // off the chips, on a gem (within its radius and the slack)
        assert_eq!(
            hit_test(
                pos2(300.0, 300.0 + 6.0 + GEM_SLACK_PX),
                &chips,
                &gems,
                Some(5)
            ),
            Hit::Gem(37)
        );
        assert_eq!(
            hit_test(
                pos2(300.0, 300.0 + 6.0 + GEM_SLACK_PX + 1.0),
                &chips,
                &gems,
                Some(5)
            ),
            Hit::Surface(5)
        );
        // the gingiva and the background
        assert_eq!(
            hit_test(pos2(500.0, 500.0), &chips, &gems, Some(0)),
            Hit::Surface(0)
        );
        assert_eq!(
            hit_test(pos2(500.0, 500.0), &chips, &gems, None),
            Hit::Nothing
        );
    }

    #[test]
    fn the_nearest_gem_wins() {
        let gems = [
            Gem {
                fdi: 14,
                center: pos2(50.0, 50.0),
                radius: 8.0,
            },
            Gem {
                fdi: 15,
                center: pos2(58.0, 50.0),
                radius: 8.0,
            },
        ];
        assert_eq!(hit_test(pos2(54.0, 50.0), &[], &gems, None), Hit::Gem(14));
        assert_eq!(hit_test(pos2(56.0, 50.0), &[], &gems, None), Hit::Gem(15));
    }

    #[test]
    fn an_alt_click_chooses_the_brush_tooth() {
        assert_eq!(alt_click_action(Hit::Label(36)), Action::SelectBrush(36));
        assert_eq!(alt_click_action(Hit::Gem(21)), Action::SelectBrush(21));
        assert_eq!(alt_click_action(Hit::Surface(14)), Action::SelectBrush(14));
        // the gingiva is the brush "0"
        assert_eq!(alt_click_action(Hit::Surface(0)), Action::SelectBrush(0));
        assert_eq!(alt_click_action(Hit::Nothing), Action::None);
    }
}
