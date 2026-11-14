//! The Aligners data in the 3D viewport: per-vertex tooth label colours
//! (the labels blob, drawn with [`LabelPalette`] as `dc-raster` does), the
//! teeth posed at the stage shown on the timeline, seed markers and the
//! occlusal contact map as a colour ramp ([`dca_algo::occlusion`]).
//!
//! Everything is read from the case state, so a command that changes it (in
//! the window or through the `--control` bridge, both land on the undo
//! stack) refreshes the scene at the next frame.

use std::collections::BTreeMap;
use std::sync::Arc;

use dc_case::{CaseState, Fdi, Pose, ScanRole};
use dc_mesh::TriMesh;
use dc_scene::{Color, LabelPalette};
use dca_algo::anatomy::Arch;
use dca_algo::occlusion::{self, ArchModel};
use dca_model::{AlignersState, Seed, Stage, TeethChange, ToothFrame};
use glam::{DVec3, Vec3};

/// Layers drawn over the scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layers {
    /// Scans coloured by tooth label (when the segmentation exists).
    pub labels: bool,
    /// Seed markers.
    pub seeds: bool,
    /// Tooth numbers (a chip per tooth).
    pub numbers: bool,
    /// Occlusal contact map instead of the label colours.
    pub contacts: bool,
    /// Ghost of the teeth at their final position (the setup), at any
    /// stage (the plugin's `ghost_final`).
    pub target: bool,
    /// The IPR planned by the final position: a mark at each contact with
    /// its amount, moved with the teeth at the stage shown.
    pub ipr: bool,
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            labels: true,
            seeds: true,
            numbers: true,
            contacts: false,
            target: false,
            ipr: false,
        }
    }
}

/// The final position of the teeth (the setup) as a stage: the pose of
/// every tooth it moves, relative to its rest pose as a stage stores it
/// (read with [`tooth_move`] and [`posed_mesh`]). `None` when the case has
/// no final position.
pub(crate) fn final_stage(al: &AlignersState) -> Option<Stage> {
    if al.setup.poses.is_empty() {
        return None;
    }
    let mut s = Stage::new(u32::MAX);
    s.poses = al.setup.poses.clone();
    Some(s)
}

/// Palette of the tooth labels: the one of `dc-raster`, so renders match.
pub(crate) fn palette() -> LabelPalette {
    LabelPalette::default()
}

/// The Aligners data of the case.
pub(crate) fn aligners(state: &CaseState) -> Option<&AlignersState> {
    state.module_data::<AlignersState>(dca_model::MODULE_ID)
}

/// The arches whose built teeth are out of date: their tooth colours, tooth
/// marks or missing teeth changed since the teeth were built, or a tooth
/// corrected by hand waits to be rebuilt
/// ([`AlignersState::teeth_out_of_date`]). The viewport draws their scan,
/// with its colours and tooth marks, instead of the 3D teeth, so the
/// corrections stay in sight until the teeth are built again.
pub(crate) fn outdated_arches(state: &CaseState) -> Vec<ScanRole> {
    let Some(a) = aligners(state) else {
        return Vec::new();
    };
    [ScanRole::Upper, ScanRole::Lower]
        .into_iter()
        .filter(|role| a.teeth_out_of_date(*role))
        .collect()
}

/// The names of the arches whose built teeth are out of date
/// ([`outdated_arches`]), as the user reads them ("Upper, Lower"), or
/// `None` when none is.
pub(crate) fn outdated_arch_names(state: &CaseState) -> Option<String> {
    let names: Vec<String> = outdated_arches(state).into_iter().map(role_name).collect();
    (!names.is_empty()).then(|| names.join(", "))
}

/// The name of the arch `role` as the user reads it ("Upper").
fn role_name(role: ScanRole) -> String {
    crate::t(if role == ScanRole::Upper {
        "ws-role-upper"
    } else {
        "ws-role-lower"
    })
}

/// The arches whose built teeth are out of date, each with what changed
/// since the teeth were built ([`AlignersState::teeth_changes`]), as the
/// user reads them ("Upper: tooth colours, tooth marks; Lower: missing
/// teeth"), or `None` when none is.
pub(crate) fn outdated_reasons(state: &CaseState) -> Option<String> {
    let a = aligners(state)?;
    let lines: Vec<String> = [ScanRole::Upper, ScanRole::Lower]
        .into_iter()
        .filter_map(|role| {
            let why: Vec<String> = a
                .teeth_changes(role)
                .into_iter()
                .map(|c| {
                    crate::t(match c {
                        TeethChange::HandCorrected => "ws-teeth-why-hand",
                        TeethChange::Colours => "ws-teeth-why-colours",
                        TeethChange::Marks => "ws-teeth-why-marks",
                        TeethChange::Missing => "ws-teeth-why-missing",
                        TeethChange::Orientation => "ws-teeth-why-orientation",
                    })
                })
                .collect();
            (!why.is_empty()).then(|| format!("{}: {}", role_name(role), why.join(", ")))
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("; "))
}

/// The arches with a scan but without their 3D teeth or gingiva while the
/// other arch has its 3D teeth, each with the i18n key of the reason: no
/// tooth colours yet (separate teeth and gum first), or not built.
pub(crate) fn unbuilt_arches(state: &CaseState) -> Vec<(ScanRole, &'static str)> {
    let Some(a) = aligners(state) else {
        return Vec::new();
    };
    let built = |role: ScanRole| {
        a.crowns
            .keys()
            .any(|f| f.is_upper() == (role == ScanRole::Upper))
            && a.gingiva.contains_key(&role)
    };
    if !built(ScanRole::Upper) && !built(ScanRole::Lower) {
        return Vec::new();
    }
    [ScanRole::Upper, ScanRole::Lower]
        .into_iter()
        .filter(|role| state.case.scan(*role).is_some() && !built(*role))
        .map(|role| {
            let key = if a.labels.contains_key(&role) {
                "ws-teeth-unbuilt-run"
            } else {
                "ws-teeth-unbuilt-separate"
            };
            (role, key)
        })
        .collect()
}

/// [`unbuilt_arches`] as the user reads them, one per line.
pub(crate) fn unbuilt_lines(state: &CaseState) -> Vec<String> {
    unbuilt_arches(state)
        .into_iter()
        .map(|(role, key)| dc_i18n::tr_args(key, &[("arch", &role_name(role))]))
        .collect()
}

/// The labels mesh of the arch `role`, when it has one label per vertex of
/// `scan`.
pub(crate) fn scan_labels<'a>(
    state: &'a CaseState,
    role: ScanRole,
    scan: &TriMesh,
) -> Option<&'a TriMesh> {
    let r = aligners(state)?.labels.get(&role)?;
    let m = state.mesh(r)?;
    m.vertex_labels
        .as_ref()
        .is_some_and(|l| l.len() == scan.vertex_count())
        .then_some(m.as_ref())
}

/// The stage with this index.
pub(crate) fn stage(al: &AlignersState, index: u32) -> Option<&Stage> {
    al.stages.iter().find(|s| s.index == index)
}

/// The move (case coordinates) of a tooth with rest frame `frame` and pose
/// `pose` in a stage: the pose is relative to the rest pose of the tooth
/// (`dca_algo::teeth::rest_pose`: the frame with its origin at the centre
/// of resistance), `rest · pose · rest⁻¹`, as the setup, the staging, the
/// export and the 3D teeth read it.
pub(crate) fn tooth_move(frame: &ToothFrame, pose: &Pose) -> Pose {
    let rest = dca_algo::teeth::rest_pose(frame);
    rest.compose(pose).compose(&rest.inverse())
}

/// `scan` with every labelled tooth moved to its pose at `stage`
/// ([`tooth_move`]). `None` when no tooth of the scan moves.
pub(crate) fn posed_mesh(
    scan: &TriMesh,
    labels: &[u16],
    frames: &BTreeMap<Fdi, ToothFrame>,
    stage: &Stage,
) -> Option<TriMesh> {
    let moves: BTreeMap<u16, Pose> = stage
        .poses
        .iter()
        .filter_map(|(f, p)| {
            let fr = frames.get(f)?;
            Some((u16::from(f.get()), tooth_move(fr, p)))
        })
        .collect();
    if moves.is_empty() || labels.len() != scan.vertex_count() {
        return None;
    }
    let mut out = scan.clone();
    let mut moved = false;
    for (p, l) in out.positions.iter_mut().zip(labels) {
        if let Some(m) = moves.get(l) {
            *p = m.apply_point(p.as_dvec3()).as_vec3();
            moved = true;
        }
    }
    if !moved {
        return None;
    }
    if out.normals.is_some() {
        out.update_normals();
    }
    Some(out)
}

/// Seed markers: a small octahedron (`radius` mm) at every seed, its
/// vertices labelled with the tooth number (drawn with the label palette).
pub(crate) fn seed_markers<'a>(
    seeds: impl IntoIterator<Item = &'a Seed>,
    radius: f32,
) -> Option<TriMesh> {
    const DIRS: [Vec3; 6] = [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ];
    // faces of the octahedron, outward (counter-clockwise)
    const FACES: [[u32; 3]; 8] = [
        [0, 2, 4],
        [2, 1, 4],
        [1, 3, 4],
        [3, 0, 4],
        [2, 0, 5],
        [1, 2, 5],
        [3, 1, 5],
        [0, 3, 5],
    ];
    let mut positions = Vec::new();
    let mut tris = Vec::new();
    let mut labels = Vec::new();
    for s in seeds {
        let base = positions.len() as u32;
        let c = s.position.as_vec3();
        positions.extend(DIRS.iter().map(|d| c + *d * radius));
        labels.extend([u16::from(s.fdi.get()); 6]);
        tris.extend(FACES.iter().map(|f| f.map(|i| base + i)));
    }
    if positions.is_empty() {
        return None;
    }
    let mut m = TriMesh::new(positions, tris);
    m.vertex_labels = Some(labels);
    Some(m)
}

/// Linear value of an sRGB channel.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Colour of a signed distance on the contact map (the plugin's sRGB scale
/// of [`occlusion::CONTACT_SCALE`], as a linear [`Color`]).
pub(crate) fn contact_color(d: f64) -> Color {
    let [r, g, b, a] = occlusion::contact_color(d);
    Color::rgba(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a)
}

/// Contact map of the two arches, as per-vertex colours of their scans.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContactColors {
    /// One colour per vertex of the lower scan.
    pub lower: Arc<Vec<Color>>,
    /// One colour per vertex of the upper scan.
    pub upper: Arc<Vec<Color>>,
    /// Deepest interpenetration (mm).
    pub penetration_mm: f64,
    /// Teeth in contact on either arch.
    pub teeth_in_contact: usize,
}

impl ContactColors {
    /// Colours of the scan of `role`.
    pub fn of(&self, role: ScanRole) -> Option<&Arc<Vec<Color>>> {
        match role {
            ScanRole::Lower => Some(&self.lower),
            ScanRole::Upper => Some(&self.upper),
            ScanRole::Bite => None,
        }
    }
}

fn arch_model(state: &CaseState, al: &AlignersState, arch: Arch) -> Result<ArchModel, String> {
    let role = arch.role();
    let scan = state
        .case
        .scan(role)
        .and_then(|s| state.mesh(&s.mesh))
        .ok_or_else(|| format!("no {role:?} scan").to_lowercase())?;
    let labels = scan_labels(state, role, scan)
        .and_then(|m| m.vertex_labels.as_ref())
        .ok_or_else(|| format!("the {role:?} scan is not segmented").to_lowercase())?;
    let v: Vec<DVec3> = scan.positions.iter().map(|p| p.as_dvec3()).collect();
    let flipped = al.occlusal_flipped.contains(&role);
    ArchModel::new(arch, &v, &scan.tris, labels, &al.seeds, flipped).map_err(|e| e.to_string())
}

/// Contact map of the case (both arches segmented with their seeds).
pub(crate) fn contact_colors(state: &CaseState) -> Result<ContactColors, String> {
    let al = aligners(state).ok_or_else(|| "no Aligners data".to_owned())?;
    let lower = arch_model(state, al, Arch::Lower)?;
    let upper = arch_model(state, al, Arch::Upper)?;
    let map = occlusion::contact_map(&lower, &upper).map_err(|e| e.to_string())?;
    let gum = palette().unlabeled;
    let colors = |m: &ArchModel, d: &[f64]| -> Arc<Vec<Color>> {
        Arc::new(
            m.labels
                .iter()
                .zip(d)
                .map(|(&l, &d)| if l == 0 { gum } else { contact_color(d) })
                .collect(),
        )
    };
    Ok(ContactColors {
        lower: colors(&lower, &map.lower.distances),
        upper: colors(&upper, &map.upper.distances),
        penetration_mm: map.penetration_mm,
        teeth_in_contact: map.lower.teeth_in_contact.len() + map.upper.teeth_in_contact.len(),
    })
}

/// Rows of the contact map legend: colour, i18n key of the band, range text.
pub(crate) fn contact_legend() -> Vec<(Color, &'static str, String)> {
    let mut rows = Vec::new();
    let mut lo: Option<f64> = None;
    for (hi, _) in occlusion::CONTACT_SCALE {
        let key = if hi <= 0.0 {
            "ws-contacts-penetration"
        } else if hi <= occlusion::CONTACT_MM + 1e-9 {
            "ws-contacts-contact"
        } else {
            "ws-contacts-near"
        };
        let range = match lo {
            None => format!("< {hi:.2} mm"),
            Some(l) => format!("{l:.2} … {hi:.2} mm"),
        };
        rows.push((
            contact_color((hi + lo.unwrap_or(hi - 1.0)) / 2.0),
            key,
            range,
        ));
        lo = Some(hi);
    }
    let last = lo.unwrap_or(0.5);
    rows.push((
        contact_color(f64::INFINITY),
        "ws-contacts-none",
        format!("≥ {last:.2} mm"),
    ));
    rows
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use dc_command::JobCtx;
    use glam::DQuat;
    use serde_json::{Value, json};

    pub(crate) fn exec(id: &str, state: &CaseState, params: Value) -> CaseState {
        dca_module::commands::find(id)
            .unwrap_or_else(|| panic!("{id}"))
            .execute(state, &params, &JobCtx::detached())
            .unwrap()
            .0
    }

    pub(crate) fn lower_demo() -> CaseState {
        exec("aligners.demo", &CaseState::default(), Value::Null)
    }

    #[test]
    fn the_labels_blob_colours_its_scan() {
        let s = lower_demo();
        let scan = s.mesh(&s.case.scan(ScanRole::Lower).unwrap().mesh).unwrap();
        let labelled = scan_labels(&s, ScanRole::Lower, scan).unwrap();
        let c = dc_viewport::label_coloring(scan, labelled, &palette()).unwrap();
        let colors = dc_viewport::resolve_colors(scan, &c);
        let labels = labelled.vertex_labels.as_ref().unwrap();
        for (k, l) in labels.iter().enumerate().step_by(97) {
            assert_eq!(colors[k], palette().color(*l));
        }
        // gingiva and teeth differ
        let gum = labels.iter().position(|&l| l == 0).unwrap();
        let tooth = labels.iter().position(|&l| l != 0).unwrap();
        assert_ne!(colors[gum], colors[tooth]);
        assert!(scan_labels(&s, ScanRole::Upper, scan).is_none());
    }

    #[test]
    fn a_stage_moves_only_the_teeth_it_poses() {
        let s = exec("aligners.teeth.frames", &lower_demo(), Value::Null);
        let al = aligners(&s).unwrap();
        let scan = s.mesh(&s.case.scan(ScanRole::Lower).unwrap().mesh).unwrap();
        let labels = scan_labels(&s, ScanRole::Lower, scan)
            .unwrap()
            .vertex_labels
            .clone()
            .unwrap();
        let f31 = Fdi::new(31).unwrap();
        let mut st = Stage::new(1);
        // 1 mm along the tooth's mesio-distal axis, plus a small turn
        st.poses.insert(
            f31,
            Pose::new(DVec3::new(1.0, 0.0, 0.0), DQuat::from_rotation_z(0.05)),
        );
        let posed = posed_mesh(scan, &labels, &al.tooth_frames, &st).unwrap();
        // the move the plan reads from the pose (about the centre of resistance)
        let frame = al.tooth_frames[&f31];
        let moving = tooth_move(&frame, &st.poses[&f31]);
        for (k, l) in labels.iter().enumerate() {
            let d = posed.positions[k].distance(scan.positions[k]);
            if *l == 31 {
                let want = moving.apply_point(scan.positions[k].as_dvec3());
                assert!(posed.positions[k].as_dvec3().distance(want) < 1e-4);
            } else {
                assert_eq!(d, 0.0, "vertex {k} of {l}");
            }
        }
        // the centre of resistance of 31 moved 1 mm along the tooth's own x axis
        let o = dca_algo::teeth::rest_pose(&frame).t;
        let moved = moving.apply_point(o);
        assert!((moved.distance(o) - 1.0).abs() < 1e-9);
        // a stage without known teeth moves nothing
        assert!(posed_mesh(scan, &labels, &BTreeMap::new(), &st).is_none());
        assert!(posed_mesh(scan, &labels, &al.tooth_frames, &Stage::new(2)).is_none());
    }

    #[test]
    fn seed_markers_carry_the_tooth_numbers() {
        let s = lower_demo();
        let seeds = &aligners(&s).unwrap().seeds;
        let m = seed_markers(seeds, 0.8).unwrap();
        assert_eq!(m.vertex_count(), seeds.len() * 6);
        assert_eq!(m.face_count(), seeds.len() * 8);
        m.check().unwrap();
        let l = m.vertex_labels.as_ref().unwrap();
        assert_eq!(l[0], u16::from(seeds[0].fdi.get()));
        // closed and outward: positive volume
        let vol: f32 = m
            .tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| m.positions[i as usize]);
                a.dot(b.cross(c)) / 6.0
            })
            .sum();
        assert!(vol > 0.0);
        assert!(seed_markers(&[], 1.0).is_none());
    }

    #[test]
    fn the_contact_map_colours_both_arches() {
        let s = exec(
            "aligners.demo",
            &lower_demo(),
            json!({ "arch": "upper", "seed": 11 }),
        );
        let closed = exec("aligners.occlusion.virtual-bite", &s, Value::Null);
        let map = contact_colors(&closed).unwrap();
        assert!(map.teeth_in_contact > 2, "{}", map.teeth_in_contact);
        let none = contact_color(f64::INFINITY);
        for colors in [&map.lower, &map.upper] {
            assert!(
                colors
                    .iter()
                    .any(|c| *c != none && *c != palette().unlabeled)
            );
        }
        assert_eq!(
            map.lower.len(),
            closed
                .mesh(&closed.case.scan(ScanRole::Lower).unwrap().mesh)
                .unwrap()
                .vertex_count()
        );
        // one arch only: no map, with the reason
        let err = contact_colors(&lower_demo()).unwrap_err();
        assert!(err.contains("upper"), "{err}");
        let mut no_data = lower_demo();
        no_data.modules.clear();
        assert!(contact_colors(&no_data).is_err());
    }

    #[test]
    fn the_contact_legend_follows_the_scale() {
        let rows = contact_legend();
        assert_eq!(rows.len(), occlusion::CONTACT_SCALE.len() + 1);
        assert_eq!(rows[0].1, "ws-contacts-penetration");
        assert_eq!(rows[4].1, "ws-contacts-contact");
        assert_eq!(rows[6].1, "ws-contacts-near");
        assert_eq!(rows[7].1, "ws-contacts-none");
        // every band has its own colour
        for w in rows.windows(2) {
            assert_ne!(w[0].0, w[1].0);
        }
        // sRGB to linear: the darkest bins stay dark, white stays white
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        assert!(srgb_to_linear(0.5) < 0.25);
    }
}
