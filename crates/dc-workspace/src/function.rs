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
}

/// A function with nothing more than its run button.
const PLAIN: Function = Function {
    id: "",
    results: None,
    params: Params::None,
    note: None,
    corrections: Corrections::None,
    region: false,
    more: None,
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
        id: leaf::STUDY_MODELS,
        params: Params::Module,
        ..PLAIN
    },
    Function {
        id: leaf::BITE,
        results: Some(Workspace::bite_results),
        params: Params::Own(Workspace::bite_params),
        corrections: Corrections::Tools(&[Tool::ThreePoints]),
        more: Some(Workspace::bite_corrections),
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
        let done = self.is_done(id);
        // a function that leaves nothing in the case and is not a step of
        // the workflow has no status to give
        if done || item.counts_as_step {
            ui.add_space(10.0);
            if done {
                widgets::badge(ui, BadgeKind::Success, t("ws-fn-done"));
            } else {
                widgets::badge(ui, BadgeKind::Neutral, t("ws-fn-not-done"));
            }
        }
        if let Some(results) = f.results {
            ui.add_space(8.0);
            results(self, ui);
        }

        match f.params {
            Params::None => {}
            Params::Module => {
                section(ui, "ws-fn-params");
                self.module_ctx_run(|m, ctx| m.inspector(ui, ctx));
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
        // once done, running again is not the main action of the page
        let button = if done && item.input.is_none() && !is_import_scans_command(id) {
            Button::secondary(label)
        } else {
            Button::primary(label)
        };
        if button
            .enabled(!self.is_busy())
            .full_width(true)
            .show(ui)
            .clicked()
        {
            self.run_leaf(id);
        }
        if let Some((_, line)) = leaf::long_notice(id) {
            ui.add_space(6.0);
            caption(ui, t(line));
        }

        match f.corrections {
            Corrections::None => {}
            Corrections::Tools(tools) => {
                let tools: Vec<Tool> = tools
                    .iter()
                    .copied()
                    .filter(|tool| self.tool_ready(*tool))
                    .collect();
                if !tools.is_empty() || f.region || f.more.is_some() {
                    section(ui, "ws-fn-manual");
                }
                if f.region {
                    self.region_controls(ui);
                    ui.add_space(8.0);
                }
                if !tools.is_empty() {
                    self.tool_list(ui, &tools);
                }
                if let Some(more) = f.more {
                    ui.add_space(8.0);
                    more(self, ui);
                }
            }
            Corrections::Own(draw) => {
                section(ui, "ws-fn-manual");
                draw(self, ui);
            }
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
    fn a_function_not_listed_has_only_its_run_button() {
        let f = of(leaf::EXPORT_ALL);
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
    fn the_aligners_show_their_result_parameters_and_slow_a_tooth() {
        let f = of(leaf::ALIGNERS);
        assert!(f.results.is_some());
        assert!(matches!(f.params, Params::Own(_)));
        // "Slow a tooth" works on the tooth selected in the view: not a pointer tool
        assert!(matches!(f.corrections, Corrections::Own(_)));
        assert!(f.note.is_none() && f.more.is_none());
    }

    #[test]
    fn the_clinical_review_shows_its_counts_and_its_decisions() {
        let f = of(leaf::REVIEW);
        assert!(f.results.is_some() && f.note.is_some());
        // a run takes no parameter: the filter and the decisions are corrections
        assert!(matches!(f.params, Params::None));
        assert!(matches!(f.corrections, Corrections::Own(_)));
    }
}
