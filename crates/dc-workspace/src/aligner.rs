//! The aligner shown in the 3D view (gap map 12.8; the plugin's
//! `show_aligner`, `props.py:715`, and `oa.aligner_exact`,
//! `ops_view_anim.py:128`, next to the playback of its Aligners page,
//! `ui_phases.py:905-909`).
//!
//! The layer "Aligner" of the view toolbar (next to the final ghost, as the
//! plugin's toggle) and the first button of "The aligner in the view", in the
//! inspector of "Aligners", show or hide it: `aligners.view.aligner` with the
//! stage of the timeline builds, for every arch, the aligner of that stage at
//! the detail of the view when none is up to date there. "Exact aligner of
//! this stage" builds the one the export writes
//! (`aligners.view.aligner-exact`). Every one runs in the background, with its
//! progress and a cancel in the status bar, as one undo step; selecting a
//! function builds nothing (menu law). The module draws the aligner at every
//! stage, moved with the teeth from the nearest one built (`dca-module`
//! `aligner_scene`); the view draws it translucent in the `model_aligner`
//! colour, never picked. After a change of the design, the teeth or the plan
//! the aligner built before is out of date: it is not drawn and the inspector
//! says so, with the button that builds it again.

use std::sync::Arc;

use dc_case::{BlobRef, CaseState, ScanRole};
use dc_module_api::{Toast, ToastLevel};
use dc_ui::fonts;
use dc_ui::icons as ic;
use dc_ui::theme::colors;
use dc_ui::widgets::Button;
use dca_algo::anatomy::Arch;
use dca_algo::schedule;
use dca_model::{AlignersState, view_key};
use egui::{RichText, Ui};
use serde_json::{Value, json};

use crate::function::caption;
use crate::leaf::Reported;
use crate::workspace::Workspace;
use crate::{overlay, t};

/// The command that shows or hides the aligner (and builds it).
pub(crate) const SHOW: &str = "aligners.view.aligner";
/// The command that builds the exact aligner of a stage.
pub(crate) const EXACT: &str = "aligners.view.aligner-exact";
/// Opacity of the aligner (the alpha of the plugin's material
/// `OA_Allineatore`).
pub(crate) const ALIGNER_OPACITY: f32 = 0.35;

/// `blob` is an aligner the case built for the view.
pub(crate) fn is_view_aligner(al: Option<&AlignersState>, blob: &BlobRef) -> bool {
    al.is_some_and(|d| d.aligner_view.bases.iter().any(|b| b.mesh == *blob))
}

/// What the view shows of the aligner of an arch at the stage shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchAligner {
    /// Not built for the case as it is.
    None,
    /// Built for an earlier design, teeth or plan: not drawn.
    OutOfDate,
    /// The aligner built at `built` (exact or the preview), drawn at stage
    /// `at` (moved with the teeth when the two differ).
    Drawn { built: u32, at: u32, exact: bool },
}

/// The arches with aligners and their last aligner.
fn arches(al: &AlignersState) -> Vec<(ScanRole, u32)> {
    [Arch::Upper, Arch::Lower]
        .into_iter()
        .filter_map(|a| {
            let n = schedule::active_stages(a, &al.stages);
            (n > 0).then(|| (a.role(), n))
        })
        .collect()
}

/// The aligner of every arch with aligners at stage `stage` of the timeline
/// (an arch past its last aligner: its last one), as the module draws it.
pub(crate) fn arch_aligners(state: &CaseState, stage: u32) -> Vec<(ScanRole, ArchAligner)> {
    let Some(al) = overlay::aligners(state) else {
        return Vec::new();
    };
    let key = view_key(state, al);
    arches(al)
        .into_iter()
        .map(|(role, n)| {
            let at = stage.min(n);
            let what = match al.aligner_view.nearest(role, &key, at) {
                Some(b) => ArchAligner::Drawn {
                    built: b.stage,
                    at,
                    exact: b.exact,
                },
                None if al.aligner_view.bases.iter().any(|b| b.arch == role) => {
                    ArchAligner::OutOfDate
                }
                None => ArchAligner::None,
            };
            (role, what)
        })
        .collect()
}

/// The line of the inspector for the aligner of an arch.
fn arch_line(role: ScanRole, a: ArchAligner) -> String {
    let arch = t(if role == ScanRole::Upper {
        "ws-role-upper"
    } else {
        "ws-role-lower"
    });
    let key = match a {
        ArchAligner::None => "ws-aln-none",
        ArchAligner::OutOfDate => "ws-aln-out-of-date",
        ArchAligner::Drawn { built, at, .. } if built != at => "ws-aln-moved",
        ArchAligner::Drawn { exact: true, .. } => "ws-aln-exact-here",
        ArchAligner::Drawn { .. } => "ws-aln-here",
    };
    let (built, at) = match a {
        ArchAligner::Drawn { built, at, .. } => (built, at),
        _ => (0, 0),
    };
    dc_i18n::tr_args(
        key,
        &[
            ("arch", &arch),
            ("stage", &at.to_string()),
            ("built", &built.to_string()),
        ],
    )
}

/// The notification of the end of a command of the view: the exact aligner
/// built, and the notes of a build (holes left, a border used instead).
pub(crate) fn report(id: &str, out: &Value) -> Option<Reported> {
    let notes: Vec<String> = out["notes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|n| n["detail"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if !notes.is_empty() {
        return Some(Reported {
            toast: Toast::new(ToastLevel::Warning, "ws-toast-aligner-notes")
                .with_detail(notes.join("\n")),
            sticky: true,
        });
    }
    let built = out["built"].as_array().filter(|b| !b.is_empty())?;
    if id != EXACT {
        return None;
    }
    let seconds = built
        .iter()
        .filter_map(|b| b["seconds"].as_f64())
        .fold(0.0, f64::max);
    let stage = out["stage"].as_u64().unwrap_or(0).to_string();
    Some(Reported {
        toast: Toast::new(ToastLevel::Success, "ws-toast-aligner-exact").with_detail(
            dc_i18n::tr_args(
                "ws-aln-exact-done",
                &[("stage", &stage), ("seconds", &format!("{seconds:.0}"))],
            ),
        ),
        sticky: false,
    })
}

impl Workspace {
    /// The case shows the aligner in the view.
    pub(crate) fn aligner_shown(&self) -> bool {
        overlay::aligners(self.undo.current()).is_some_and(|a| a.aligner_view.shown)
    }

    /// The case has aligners to show.
    fn has_aligners(&self) -> bool {
        overlay::aligners(self.undo.current()).is_some_and(|a| !arches(a).is_empty())
    }

    /// The aligner on or off (the layer "Aligner" and the first button of
    /// the inspector). On, the aligner of the stage shown is built when none
    /// is up to date there; off, the aligners built for the view go. A case
    /// without aligners keeps it off and says so.
    pub(crate) fn toggle_aligner_layer(&mut self) {
        if self.aligner_shown() {
            self.run_button_command(SHOW, json!({ "show": false }), "ws-job-aligner-hide");
        } else if self.has_aligners() {
            self.build_view_aligner();
        } else {
            self.toast(Toast::new(ToastLevel::Info, "ws-toast-aligner-none"));
        }
    }

    /// The aligner of the stage shown, built at the detail of the view where
    /// none is up to date.
    fn build_view_aligner(&mut self) {
        let stage = self.timeline.current;
        self.run_button_command(
            SHOW,
            json!({ "show": true, "stage": stage }),
            "ws-job-aligner",
        );
    }

    /// "Exact aligner of this stage": the aligner the export writes, for the
    /// stage shown.
    fn build_exact_aligner(&mut self) {
        let stage = self.timeline.current;
        self.run_button_command(EXACT, json!({ "stage": stage }), "ws-job-aligner-exact");
    }

    /// "The aligner in the view" in the inspector of "Aligners": show or
    /// hide it, the exact aligner of the stage shown, what each arch shows
    /// and, when it is out of date, the button that builds it again.
    pub(crate) fn aligner_view_controls(&mut self, ui: &mut Ui) {
        let c = *colors(ui);
        ui.add_space(12.0);
        ui.label(
            RichText::new(t("ws-aln-view"))
                .font(fonts::semibold(13.0))
                .color(c.text_primary),
        );
        caption(ui, t("ws-aln-view-hint"));
        ui.add_space(6.0);
        let state = Arc::clone(self.undo.current());
        let has = self.has_aligners();
        let shown = self.aligner_shown();
        let busy = self.is_busy();
        let arches = arch_aligners(&state, self.timeline.current);
        let stale = shown
            && arches
                .iter()
                .any(|(_, a)| !matches!(a, ArchAligner::Drawn { .. }));
        // the exact aligner and what each drawn arch shows are advanced
        // options; showing or hiding it, and building again an aligner out
        // of date (with the line that says so), are not
        let advanced = dc_module_api::advanced_ui(ui.ctx());
        let (mut toggle, mut exact) = (false, false);
        let toggle_button = Button::secondary(t(if shown { "ws-aln-hide" } else { "ws-aln-show" }))
            .icon(ic::CUBE_TRANSPARENT)
            .selected(shown)
            .enabled(!busy && (shown || has))
            .full_width(true);
        if advanced {
            ui.columns(2, |cols| {
                toggle = toggle_button.show(&mut cols[0]).clicked();
                exact = Button::secondary(t("ws-aln-exact"))
                    .icon(ic::TARGET)
                    .enabled(!busy && has)
                    .full_width(true)
                    .show(&mut cols[1])
                    .clicked();
            });
        } else {
            toggle = toggle_button.show(ui).clicked();
        }
        let rebuild = stale
            && Button::secondary(t("ws-aln-rebuild"))
                .icon(ic::ARROW_CLOCKWISE)
                .enabled(!busy)
                .full_width(true)
                .show(ui)
                .clicked();
        ui.add_space(4.0);
        if !has {
            caption(ui, t("ws-aln-no-aligners"));
        } else if !shown {
            caption(ui, t("ws-aln-hidden"));
        } else {
            for (role, a) in arches {
                if advanced || !matches!(a, ArchAligner::Drawn { .. }) {
                    caption(ui, arch_line(role, a));
                }
            }
        }
        if toggle {
            self.toggle_aligner_layer();
        }
        if exact {
            self.build_exact_aligner();
        }
        if rebuild {
            self.build_view_aligner();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_case::{Fdi, ModuleState, Pose};
    use dca_model::{AlignerBase, Stage};
    use glam::{DQuat, DVec3};

    /// Both arches with aligners: the upper one moves up to 4, the lower one
    /// up to 2.
    fn staged() -> (CaseState, AlignersState) {
        let mut al = AlignersState::default();
        let moved = |n: u32| Pose::new(DVec3::new(0.1 * f64::from(n), 0.0, 0.0), DQuat::IDENTITY);
        for n in 1..=4 {
            let mut st = Stage::new(n);
            st.poses.insert(Fdi::new(11).unwrap(), moved(n));
            st.poses.insert(Fdi::new(31).unwrap(), moved(n.min(2)));
            al.stages.push(st);
        }
        (CaseState::default(), al)
    }

    fn base(arch: ScanRole, stage: u32, exact: bool, key: &str) -> AlignerBase {
        AlignerBase {
            arch,
            stage,
            mesh: BlobRef::of_bytes(format!("{arch:?}{stage}{exact}").as_bytes()),
            exact,
            voxel_mm: 0.3,
            tolerance_mm: 0.04,
            key: key.to_owned(),
            seconds: 1.0,
        }
    }

    fn with(mut s: CaseState, al: AlignersState) -> CaseState {
        s.set_module(dca_model::MODULE_ID, ModuleState::new(al));
        s
    }

    #[test]
    fn each_arch_says_which_aligner_it_draws_or_that_it_is_out_of_date() {
        let (s, mut al) = staged();
        // nothing built yet
        let st = with(s.clone(), al.clone());
        assert_eq!(
            arch_aligners(&st, 3),
            vec![
                (ScanRole::Upper, ArchAligner::None),
                (ScanRole::Lower, ArchAligner::None)
            ]
        );
        let key = view_key(&st, &al);
        al.aligner_view.shown = true;
        al.aligner_view.bases = vec![
            base(ScanRole::Upper, 1, false, &key),
            base(ScanRole::Upper, 3, true, &key),
            base(ScanRole::Lower, 2, false, "an earlier design"),
        ];
        let st = with(s, al.clone());
        // the view itself is not part of the key
        assert_eq!(view_key(&st, &al), key);
        let got = arch_aligners(&st, 3);
        assert_eq!(
            got[0],
            (
                ScanRole::Upper,
                ArchAligner::Drawn {
                    built: 3,
                    at: 3,
                    exact: true
                }
            )
        );
        assert_eq!(got[1], (ScanRole::Lower, ArchAligner::OutOfDate));
        // past its last aligner, an arch shows its last one
        let got = arch_aligners(&st, 9);
        assert_eq!(
            got[0].1,
            ArchAligner::Drawn {
                built: 3,
                at: 4,
                exact: true
            }
        );
        assert!(is_view_aligner(Some(&al), &al.aligner_view.bases[0].mesh));
        assert!(!is_view_aligner(Some(&al), &BlobRef::of_bytes(b"other")));
        assert!(!is_view_aligner(None, &al.aligner_view.bases[0].mesh));
    }

    #[test]
    fn the_lines_read_what_each_arch_shows() {
        crate::loader();
        let here = arch_line(
            ScanRole::Upper,
            ArchAligner::Drawn {
                built: 3,
                at: 3,
                exact: false,
            },
        );
        let moved = arch_line(
            ScanRole::Lower,
            ArchAligner::Drawn {
                built: 1,
                at: 4,
                exact: false,
            },
        );
        assert!(here.contains('3'), "{here}");
        assert!(moved.contains('1') && moved.contains('4'), "{moved}");
        assert_ne!(arch_line(ScanRole::Upper, ArchAligner::OutOfDate), here);
    }

    #[test]
    fn the_exact_aligner_and_the_notes_of_a_build_are_reported() {
        crate::loader();
        let out = json!({
            "stage": 3,
            "built": [{ "arch": "upper", "seconds": 7.6 }, { "arch": "lower", "seconds": 8.1 }],
            "notes": [],
        });
        let r = report(EXACT, &out).unwrap();
        assert_eq!(r.toast.message_key, "ws-toast-aligner-exact");
        assert!(r.toast.detail.as_deref().unwrap().contains('3'));
        // the preview says nothing when it went well
        assert!(report(SHOW, &out).is_none());
        // nothing built: nothing to say
        assert!(report(EXACT, &json!({ "stage": 3, "built": [], "notes": [] })).is_none());
        let notes =
            json!({ "built": [{}], "notes": [{ "detail": "2 holes through the wall left" }] });
        let r = report(SHOW, &notes).unwrap();
        assert_eq!(r.toast.level, ToastLevel::Warning);
        assert!(r.sticky);
    }
}
