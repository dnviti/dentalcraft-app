//! The print popup of the workspace ([`dc_print_ui::PrintPopup`]): a leaf of
//! the module whose command answers with `print_parts` (Aligners: "Print",
//! `aligners.production.print`) opens it with those parts once its run
//! ends, from the inspector's button or from an agent. The leaf only lists
//! the parts; the popup prepares the plates and saves the files.
//!
//! When the popup has saved print files, the parts they carry are recorded
//! as sent to the printer (`aligners.print.mark`, one undoable change; a
//! command already running makes it wait its turn, so no record is lost).

use dc_print_ui::{PrintEvent, PrintRequest};
use serde_json::{Value, json};

use crate::leaf;
use crate::workspace::Workspace;

impl Workspace {
    /// Opens the print popup when a leaf's output carries `print_parts`.
    pub(crate) fn open_print_from(&mut self, out: &Value) {
        if let Some(request) = PrintRequest::from_output(out) {
            self.print.open(request);
        }
    }

    /// `true` while the print popup is open.
    pub fn is_print_open(&self) -> bool {
        self.print.is_open()
    }

    /// Moves the popup on and records the files it saved. Called by
    /// [`Workspace::poll`].
    pub(crate) fn poll_print(&mut self) {
        self.print.poll();
        for e in self.print.take_events() {
            match e {
                PrintEvent::Saved { ids, files } => {
                    let job = files
                        .iter()
                        .map(|f| f.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.print_marks.push_back(json!({
                        "ids": ids,
                        "status": "sent",
                        "job": job,
                        "by": "app",
                    }));
                }
            }
        }
        if !self.print_marks.is_empty()
            && !self.command_running()
            && self.module_spec(leaf::PRINT_MARK).is_some()
            && let Some(params) = self.print_marks.pop_front()
        {
            self.run_button_command(leaf::PRINT_MARK, params, "ws-job-command");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> Workspace {
        Workspace::new_headless(Box::new(dca_module::AlignersModule::new()), None, None)
    }

    #[test]
    fn a_leaf_output_with_parts_to_print_opens_the_popup_with_them() {
        let mut ws = workspace();
        ws.open_print_from(&json!({"done": true}));
        assert!(!ws.is_print_open(), "an output without parts opens nothing");
        ws.open_print_from(&json!({
            "print_parts": [
                {"id": "SUP-03", "label": "SUP 03", "path": "/x/SUP_03.stl", "status": "missing"},
                {"id": "INF-03", "label": "INF 03", "path": null, "status": "missing"},
            ],
            "export_dir": "/x",
            "printed": 0,
            "missing": 2,
        }));
        assert!(ws.is_print_open());
        let parts: Vec<(&str, bool)> = ws
            .print
            .parts()
            .map(|(p, on)| (p.id.as_str(), on))
            .collect();
        assert_eq!(parts, [("SUP-03", true), ("INF-03", false)]);
    }
}
