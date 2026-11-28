//! The functions of the menu and their inspector, by the menu law: the menu
//! is grouped by function, a click (or a key) on a function only selects it
//! and shows its inspector, and the inspector holds everything about it, in
//! the same order for every function:
//!
//! 1. its title and description;
//! 2. its status (done or not yet) and what its last run left in the case;
//! 3. "Parameters": what the run takes;
//! 4. "Automatic": the button that runs it ("Run", "Run again" once done, or
//!    the dialog it needs first), the only way to run it from the window;
//! 5. "Manual corrections": its tools, the controls of the open one under
//!    them (the viewport takes the pointer for that tool). The 3D teeth start
//!    this section with the view of the region of a tooth (`region`): it only
//!    changes what the viewport shows.
//!
//! [`FUNCTIONS`] says what each function has; one that is not listed has its
//! title, status and run button only. A new function adds its entry here
//! rather than a special case in the drawing.
//!
//! With the advanced options off (`dc_module_api::advanced_ui`, the default)
//! the table also says which parts are hidden: a section whose content is
//! all advanced loses its title too, so no empty section is left. The
//! drawing functions hide the advanced controls of a section that also has
//! essential ones. Hidden is only hidden: the values and the commands stay.

use dc_module_api::{MenuItem, MenuStatus, NOT_AVAILABLE_KEY, is_import_scans_command};
use dc_ui::fonts;
use dc_ui::theme::colors;
use dc_ui::widgets::{self, BadgeKind, Button};
use egui::{RichText, Ui};

use crate::leaf;
use crate::manual::Tool;
use crate::t;
use crate::workspace::Workspace;

/// A part of an inspector the workspace draws.
type Draw = fn(&mut Workspace, &mut Ui);
/// A part of an inspector the workspace draws when it applies (`true`).
type Draw2 = fn(&mut Workspace, &mut Ui) -> bool;

/// Who draws the parameters of a function.
#[derive(Clone, Copy)]
enum Params {
    /// It has none.
    None,
    /// The module, in its inspector: they are settings of its data.
    Module,
    /// The workspace.
    Own(Draw),
}

/// What of a function is corrected by hand.
#[derive(Clone, Copy)]
enum Corrections {
    /// Nothing.
    None,
    /// Tools that work with the pointer on the case.
    Tools(&'static [Tool]),
    /// Controls of their own.
    Own(Draw),
}

/// What the inspector of a function shows besides its title, its status and
/// its run button.
struct Function {
    /// The leaf of the menu (its command id).
    id: &'static str,
    /// Under the status: what its last run left in the case.
    results: Option<Draw>,
    /// The section "Parameters".
    params: Params,
    /// A line above the run button about what a run does.
    note: Option<&'static str>,
    /// The section "Manual corrections".
    corrections: Corrections,
    /// The section "Manual corrections" starts with the controls of the view
    /// of a tooth region on the scan (`region`).
    region: bool,
    /// Controls of their own after the tools of the section "Manual
    /// corrections" (a correction that is a button, not a pointer tool).
    more: Option<Draw>,
    /// The section "Parameters" holds only advanced options.
    advanced_params: bool,
    /// The corrections of [`Corrections::Own`] are only advanced options.
    advanced_corrections: bool,
    /// [`Function::more`] is only advanced options.
    advanced_more: bool,
    /// Draws, when the function does not apply to the case, what the
    /// inspector shows in place of its status, parameters, run button and
    /// corrections, and returns `true`; `false` (nothing drawn): it applies.
    not_applicable: Option<Draw2>,
}

/// The tools shown only with the advanced options on (the region view of
/// [`Function::region`] is too).
const ADVANCED_TOOLS: &[Tool] = &[Tool::Renumber, Tool::Missing];

/// A function with nothing more than its run button.
const PLAIN: Function = Function {
    id: "",
    results: None,
    params: Params::None,
    note: None,
    corrections: Corrections::None,
    region: false,
    more: None,
    advanced_params: false,
    advanced_corrections: false,
    advanced_more: false,
    not_applicable: None,
};

/// "Import scans", whatever the id of the module's import: the role of the
/// next files, and the roles of the scans of the case to correct.
const IMPORT: Function = Function {
    params: Params::Own(Workspace::import_role_params),
    corrections: Corrections::Own(Workspace::import_scan_list),
    ..PLAIN
};

/// The functions with more than a run button.
static FUNCTIONS: &[Function] = &[
    Function {
        id: leaf::FIND_TEETH,
        results: Some(Workspace::marks_results),
        params: Params::Module,
        corrections: Corrections::Tools(&[
            Tool::Seeds,
            Tool::Delete,
            Tool::Renumber,
            Tool::Missing,
        ]),
        ..PLAIN
    },
    Function {
        id: leaf::SEGMENT,
        results: Some(Workspace::segment_results),
        params: Params::Own(Workspace::segment_params),
        note: Some("ws-seg-info"),
        corrections: Corrections::Tools(&[Tool::Brush, Tool::Clean]),
        // the three numbers of the separation
        advanced_params: true,
        ..PLAIN
    },
    Function {
        id: leaf::BUILD_TEETH,
        results: Some(Workspace::teeth_results),
        corrections: Corrections::Tools(&[Tool::Rebuild]),
        region: true,
        ..PLAIN
    },
    Function {
        id: leaf::CASE_ANALYSIS,
        results: Some(Workspace::case_analysis_results),
        corrections: Corrections::Own(Workspace::case_analysis_corrections),
        // the figures set by hand
        advanced_corrections: true,
        ..PLAIN
    },
    Function {
        id: leaf::SETUP,
        results: Some(Workspace::setup_results),
        params: Params::Own(Workspace::setup_params),
        corrections: Corrections::Tools(&[Tool::Adjust, Tool::ArchLine]),
        more: Some(Workspace::setup_corrections),
        ..PLAIN
    },
    Function {
        id: leaf::ALIGNERS,
        results: Some(Workspace::staging_results),
        params: Params::Own(Workspace::staging_params),
        corrections: Corrections::Own(Workspace::staging_corrections),
        // "Slow a tooth"
        advanced_corrections: true,
        // the aligner in the view, next to the playback as in the plugin
        more: Some(Workspace::aligner_view_controls),
        ..PLAIN
    },
    Function {
        id: leaf::REVIEW,
        results: Some(Workspace::review_results),
        note: Some("ws-rev-info"),
        corrections: Corrections::Own(Workspace::review_corrections),
        ..PLAIN
    },
    Function {
        id: leaf::EXPORT_ALL,
        params: Params::Own(Workspace::production_params),
        ..PLAIN
    },
    Function {
        id: leaf::PRINT,
        // the tracking of the module: how many aligners are printed and
        // still to print, the list with the marks the user may give
        params: Params::Module,
        ..PLAIN
    },
    Function {
        id: leaf::STUDY_MODELS,
        params: Params::Module,
        // the style and the sizes of the bases
        advanced_params: true,
        ..PLAIN
    },
    Function {
        id: leaf::BITE,
        results: Some(Workspace::bite_results),
        params: Params::Own(Workspace::bite_params),
        corrections: Corrections::Tools(&[Tool::ThreePoints]),
        more: Some(Workspace::bite_corrections),
        // "Correct the interpenetration"
        advanced_more: true,
        // one arch has no bite: nothing to register or estimate
        not_applicable: Some(Workspace::bite_not_applicable),
        ..PLAIN
    },
];

/// What the inspector of leaf `id` shows.
fn of(id: &str) -> &'static Function {
    if is_import_scans_command(id) {
        return &IMPORT;
    }
    FUNCTIONS.iter().find(|f| f.id == id).unwrap_or(&PLAIN)
}

/// The function whose inspector holds `tool`.
pub(crate) fn owner(tool: Tool) -> Option<&'static str> {
    FUNCTIONS
        .iter()
        .find(|f| matches!(f.corrections, Corrections::Tools(tools) if tools.contains(&tool)))
        .map(|f| f.id)
}

/// The function a command corrects by hand: the tools of its inspector, and
/// the corrections that are not tools (the numbering in order and the marks
/// cleared, the interpenetration corrected, the values of the analysis, the
/// final position taken back, a tooth slowed, the decisions of the review).
/// The corrections of "Import scans" (an arch chosen by hand, a scan removed
/// or closed) are not: the module itself clears what depended on a scan.
/// A new correction that is not a tool is listed here.
pub(crate) fn corrected(command: &str) -> Option<&'static str> {
    if let Some(f) = Tool::from_command(command).and_then(owner) {
        return Some(f);
    }
    match command {
        crate::manual::SEQUENCE | crate::manual::CLEAR_SEEDS => Some(leaf::FIND_TEETH),
        crate::bite::CORRECT => Some(leaf::BITE),
        leaf::DIAGNOSIS_EDIT => Some(leaf::CASE_ANALYSIS),
        crate::setup::RESET => Some(leaf::SETUP),
        crate::staging::TOOTH_SPEED_CMD => Some(leaf::ALIGNERS),
        crate::review::DECIDE | crate::review::SET_VALUE | crate::review::BULK => {
            Some(leaf::REVIEW)
        }
        _ => None,
    }
}

/// The title of a section of the inspector, under a hairline.
fn section(ui: &mut Ui, key: &str) {
    let c = *colors(ui);
    ui.add_space(16.0);
    widgets::hairline(ui);
    ui.add_space(10.0);
    ui.label(
        RichText::new(t(key))
            .font(fonts::semibold(13.0))
            .color(c.text_primary),
    );
    ui.add_space(6.0);
}

/// A secondary line of the inspector.
pub(crate) fn caption(ui: &mut Ui, text: impl Into<String>) {
    let c = *colors(ui);
    ui.label(
        RichText::new(text.into())
            .font(fonts::ui(12.0))
            .color(c.text_secondary),
    );
}

impl Workspace {
    /// The inspector of the function `item` (a leaf of the menu), in the
    /// order of the menu law (see the module documentation).
    pub(crate) fn function_inspector(&mut self, ui: &mut Ui, item: &MenuItem) {
        let Some(id) = item.command_id() else { return };
        let desc = self.commands.iter().find(|d| d.id == id);
        let ready = desc.is_some_and(|d| d.status == MenuStatus::Ready);
        let desc_key = desc.and_then(|d| d.tooltip_key);
        ui.heading(t(item.label_key));
        if let Some(key) = desc_key {
            ui.label(t(key));
        }
        if !ready {
            ui.add_space(8.0);
            caption(ui, t(NOT_AVAILABLE_KEY));
            return;
        }
        let f = of(id);
        if let Some(draw) = f.not_applicable
            && draw(self, ui)
        {
            return;
        }
        let done = self.is_done(id);
        let next = self.next_step() == Some(id);
        let advanced = dc_module_api::advanced_ui(ui.ctx());
        // a function that leaves nothing in the case and is not a step of
        // the workflow has no status to give
        if done || item.counts_as_step {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if done {
                    widgets::badge(ui, BadgeKind::Success, t("ws-fn-done"));
                } else {
                    widgets::badge(ui, BadgeKind::Neutral, t("ws-fn-not-done"));
                }
                // what to do now to reach the aligners
                if next {
                    widgets::badge(ui, BadgeKind::Info, t("ws-next-step"));
                }
            });
        }
        if let Some(results) = f.results {
            ui.add_space(8.0);
            results(self, ui);
        }

        match f.params {
            Params::None => {}
            _ if f.advanced_params && !advanced => {}
            Params::Module => {
                section(ui, "ws-fn-params");
                self.module_inspector(ui);
            }
            Params::Own(draw) => {
                section(ui, "ws-fn-params");
                draw(self, ui);
            }
        }

        section(ui, "ws-fn-automatic");
        if let Some(note) = f.note {
            caption(ui, t(note));
            ui.add_space(8.0);
        }
        let label = t(leaf::button_key(id, item.input, done));
        let label_w = ui
            .painter()
            .layout_no_wrap(
                label.clone(),
                fonts::semibold(13.0),
                egui::Color32::PLACEHOLDER,
            )
            .size()
            .x
            + 32.0;
        // the main action of the page while it is still to do (the next step
        // first of all); once done, running again is not
        let primary = next || !done || item.input.is_some() || is_import_scans_command(id);
        let button = if primary {
            Button::primary(label)
        } else {
            Button::secondary(label)
        };
        let enabled = !self.is_busy();
        if is_import_scans_command(id) && item.scanner.is_some() {
            // the files, and beside them the scan with a scanner (alpha), as
            // the module declares it: its button only opens the scan window,
            // which starts nothing by itself (under it when the two do not
            // fit on one line)
            let scan_w = dc_scan_ui::scan_button_width(ui);
            let run_w = ui.available_width() - scan_w - 8.0;
            let (run, scan) = if label_w <= run_w {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let run = button.enabled(enabled).min_width(run_w).show(ui).clicked();
                    (run, dc_scan_ui::scan_button(ui, enabled))
                })
                .inner
            } else {
                let run = button.enabled(enabled).full_width(true).show(ui).clicked();
                ui.add_space(6.0);
                (run, dc_scan_ui::scan_button(ui, enabled))
            };
            if run {
                self.run_leaf(id);
            }
            if scan {
                self.open_scan();
            }
        } else if button.enabled(enabled).full_width(true).show(ui).clicked() {
            self.run_leaf(id);
        }
        if let Some((_, line)) = leaf::long_notice(id) {
            ui.add_space(6.0);
            caption(ui, t(line));
        }

        // what the section "Manual corrections" shows: with the advanced
        // options off only its essential parts, and no title when none is left
        let tools: Vec<Tool> = match f.corrections {
            Corrections::Tools(tools) => tools
                .iter()
                .copied()
                .filter(|tool| self.tool_ready(*tool))
                // an advanced tool still open (from a command) stays closable
                .filter(|tool| {
                    advanced || !ADVANCED_TOOLS.contains(tool) || self.manual.tool == Some(*tool)
                })
                .collect(),
            _ => Vec::new(),
        };
        let own = match f.corrections {
            Corrections::Own(draw) if advanced || !f.advanced_corrections => Some(draw),
            _ => None,
        };
        // the view of a region is a check: advanced, unless it is on
        let region = f.region && (advanced || self.region_tooth().is_some());
        let more = f.more.filter(|_| advanced || !f.advanced_more);
        if !tools.is_empty() || own.is_some() || region || more.is_some() {
            section(ui, "ws-fn-manual");
        }
        if region {
            self.region_controls(ui);
            ui.add_space(8.0);
        }
        if !tools.is_empty() {
            self.tool_list(ui, &tools);
        }
        if let Some(draw) = own {
            draw(self, ui);
        }
        if let Some(more) = more {
            ui.add_space(8.0);
            more(self, ui);
        }
    }

    /// The tools of a function as buttons (a click opens one, a second click
    /// closes it), and the controls of the open one.
    fn tool_list(&mut self, ui: &mut Ui, tools: &[Tool]) {
        for &tool in tools {
            let open = self.manual.tool == Some(tool);
            let id = tool.command_id();
            let tip = self
                .commands
                .iter()
                .find(|d| d.id == id)
                .and_then(|d| d.tooltip_key)
                .map(|key| match self.shortcut_text.get(id) {
                    Some(k) => format!("{} ({k})", t(key)),
                    None => t(key),
                });
            let r = Button::secondary(t(tool.title_key()))
                .selected(open)
                .full_width(true)
                .show(ui);
            let r = match tip {
                Some(tip) => r.on_hover_text(tip),
                None => r,
            };
            if r.clicked() {
                if open {
                    self.close_tool();
                } else {
                    self.open_tool(tool);
                }
            }
            ui.add_space(4.0);
        }
        if let Some(tool) = self.manual.tool.filter(|t| tools.contains(t)) {
            ui.add_space(8.0);
            self.tool_controls(ui, tool);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_belongs_to_one_function() {
        for tool in [
            Tool::Brush,
            Tool::Clean,
            Tool::Seeds,
            Tool::Delete,
            Tool::Renumber,
            Tool::Missing,
            Tool::Rebuild,
            Tool::Adjust,
            Tool::ThreePoints,
            Tool::ArchLine,
        ] {
            let owners = FUNCTIONS
                .iter()
                .filter(|f| matches!(f.corrections, Corrections::Tools(ts) if ts.contains(&tool)))
                .count();
            assert_eq!(owners, 1, "{tool:?}");
        }
        assert_eq!(owner(Tool::Brush), Some(leaf::SEGMENT));
        assert_eq!(owner(Tool::Seeds), Some(leaf::FIND_TEETH));
        assert_eq!(owner(Tool::Delete), Some(leaf::FIND_TEETH));
        assert_eq!(owner(Tool::Rebuild), Some(leaf::BUILD_TEETH));
        assert_eq!(owner(Tool::Adjust), Some(leaf::SETUP));
        assert_eq!(owner(Tool::ThreePoints), Some(leaf::BITE));
        assert_eq!(owner(Tool::ArchLine), Some(leaf::SETUP));
    }

    #[test]
    fn a_correction_names_the_function_it_corrects() {
        // the tools, by their command
        assert_eq!(corrected(crate::manual::SEEDS), Some(leaf::FIND_TEETH));
        assert_eq!(corrected(crate::manual::BRUSH), Some(leaf::SEGMENT));
        assert_eq!(corrected(crate::archline::LINE), Some(leaf::SETUP));
        // the corrections that are buttons
        assert_eq!(corrected(crate::manual::SEQUENCE), Some(leaf::FIND_TEETH));
        assert_eq!(corrected(crate::bite::CORRECT), Some(leaf::BITE));
        assert_eq!(corrected(crate::review::BULK), Some(leaf::REVIEW));
        // no correction: a setting, a run, the corrections of the import
        for id in [
            crate::setup::SETTINGS,
            leaf::SEGMENT,
            leaf::SET_ARCH,
            leaf::MAKE_BASE,
            "case.remove-scan",
        ] {
            assert_eq!(corrected(id), None, "{id}");
        }
    }

    #[test]
    fn a_function_not_listed_has_only_its_run_button() {
        let f = of(leaf::VISITS);
        assert!(f.results.is_none() && f.note.is_none() && f.more.is_none());
        assert!(matches!(f.params, Params::None));
        assert!(matches!(f.corrections, Corrections::None));
        // the import of any module has its role and its scans
        let f = of("other.case.import-scans");
        assert!(matches!(f.params, Params::Own(_)));
        assert!(matches!(f.corrections, Corrections::Own(_)));
        // no two entries for one function
        for (i, f) in FUNCTIONS.iter().enumerate() {
            assert!(FUNCTIONS[..i].iter().all(|o| o.id != f.id), "{}", f.id);
        }
    }

    #[test]
    fn the_bite_shows_its_report_its_scans_and_its_corrections() {
        let f = of(leaf::BITE);
        assert!(f.results.is_some());
        assert!(matches!(f.params, Params::Own(_)));
        assert!(matches!(f.corrections, Corrections::Tools(t) if t == [Tool::ThreePoints]));
        assert!(f.more.is_some());
    }

    #[test]
    fn the_final_position_shows_its_result_parameters_and_corrections() {
        let f = of(leaf::SETUP);
        assert!(f.results.is_some());
        assert!(matches!(f.params, Params::Own(_)));
        assert!(
            matches!(f.corrections, Corrections::Tools(t) if t == [Tool::Adjust, Tool::ArchLine])
        );
        // "Back to the initial position" is a button after the tools
        assert!(f.more.is_some());
    }

    #[test]
    fn the_aligners_show_their_result_parameters_slow_a_tooth_and_the_aligner_in_the_view() {
        let f = of(leaf::ALIGNERS);
        assert!(f.results.is_some());
        assert!(matches!(f.params, Params::Own(_)));
        // "Slow a tooth" works on the tooth selected in the view: not a pointer tool
        assert!(matches!(f.corrections, Corrections::Own(_)));
        assert!(f.note.is_none());
        // after it, "The aligner in the view" (the plugin's toggle and
        // "Allineatore esatto di questo stadio" next to the playback)
        assert!(f.more.is_some());
    }

    #[test]
    fn the_clinical_review_shows_its_counts_and_its_decisions() {
        let f = of(leaf::REVIEW);
        assert!(f.results.is_some() && f.note.is_some());
        // a run takes no parameter: the filter and the decisions are corrections
        assert!(matches!(f.params, Params::None));
        assert!(matches!(f.corrections, Corrections::Own(_)));
    }

    #[test]
    fn export_for_printing_shows_the_production_parameters() {
        let f = of(leaf::EXPORT_ALL);
        assert!(matches!(f.params, Params::Own(_)));
        // the aligner end and the settings are parameters of the export:
        // nothing of it is corrected by hand
        assert!(matches!(f.corrections, Corrections::None));
        assert!(f.results.is_none() && f.note.is_none());
    }

    #[test]
    fn print_shows_the_tracking_of_the_module() {
        let f = of(leaf::PRINT);
        // essential: the count and the marks show in the simple interface
        assert!(matches!(f.params, Params::Module) && !f.advanced_params);
        assert!(matches!(f.corrections, Corrections::None));
        assert!(f.results.is_none() && f.note.is_none() && f.more.is_none());
    }
}
