//! The vendor line under the scanner status: what the scanner's vendor layer
//! did (fill light, fast exposure, origin of the calibration), the same on
//! every platform ([`dc_scanner::VendorStatus`], `scan.status.vendor`).

use dc_scanner::VendorStatus;
use dc_ui::fonts;
use dc_ui::theme::colors;
use dc_ui::widgets::{self, BadgeKind};
use egui::{RichText, Ui};

use crate::{ScanFailure, t};

/// The vendor status as the popup shows it.
#[derive(Debug, PartialEq)]
pub(super) struct VendorLine {
    /// "Light on · Fast exposure applied · Calibration: factory (fx …)".
    pub text: String,
    /// The notes of the device (untranslated), one per line; empty when it
    /// gave none.
    pub notes: String,
    /// The fill light or the fast exposure failed.
    pub error: bool,
    /// The calibration is the field-of-view estimate.
    pub estimate: bool,
}

/// The sentence of one setting (`light` or `fast`) in its `state`.
fn setting(key: &str, name_key: &str, state: &str) -> (String, bool) {
    if let Some(code) = state.strip_prefix("error:") {
        let why = t(&ScanFailure::new(code, "").key());
        (
            dc_i18n::tr_args(&format!("scan-vendor-{key}-error"), &[("why", &why)]),
            true,
        )
    } else if matches!(
        (key, state),
        ("light", "on" | "off" | "unsupported")
            | ("fast", "applied" | "pending" | "off" | "unsupported")
    ) {
        (t(&format!("scan-vendor-{key}-{state}")), false)
    } else {
        // a state of a newer core: shown as it is
        let name = t(name_key);
        (
            dc_i18n::tr_args("scan-vendor-other", &[("name", &name), ("state", state)]),
            false,
        )
    }
}

impl VendorLine {
    pub fn of(v: &VendorStatus) -> Self {
        let (light, light_err) = setting("light", "scan-vendor-name-light", &v.fill_light);
        let (fast, fast_err) = setting("fast", "scan-vendor-name-fast", &v.fast_exposure);
        let source = match v.calibration {
            "user" | "factory" | "factory-cached" | "estimate" => {
                t(&format!("scan-vendor-cal-{}", v.calibration))
            }
            other => other.to_owned(),
        };
        let cal = dc_i18n::tr_args(
            "scan-vendor-cal",
            &[
                ("source", &source),
                ("fx", &format!("{:.1}", v.fx)),
                ("cx", &format!("{:.1}", v.cx)),
                ("cy", &format!("{:.1}", v.cy)),
            ],
        );
        let notes = [
            (&light, &v.fill_light_note),
            (&fast, &v.fast_exposure_note),
            (&cal, &v.calibration_note),
        ]
        .into_iter()
        .filter_map(|(what, note)| note.as_ref().map(|n| format!("{what}: {n}")))
        .collect::<Vec<_>>()
        .join("\n");
        Self {
            text: format!("{light} · {fast} · {cal}"),
            notes,
            error: light_err || fast_err,
            estimate: v.calibration == "estimate",
        }
    }
}

/// Draws the vendor line, with the notes as hover text, and a warning badge
/// when the calibration is the estimate or a setting failed. The line is for
/// the advanced view; the warnings are always shown.
pub(super) fn row(ui: &mut Ui, v: &VendorStatus) {
    let c = *colors(ui);
    let line = VendorLine::of(v);
    let advanced = dc_module_api::advanced_ui(ui.ctx());
    if !advanced && !line.estimate && !line.error {
        return;
    }
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        if advanced {
            let label = ui.label(
                RichText::new(&line.text)
                    .font(fonts::ui(13.0))
                    .color(c.text_secondary),
            );
            if !line.notes.is_empty() {
                label.on_hover_text(&line.notes);
            }
        }
        if line.estimate {
            widgets::badge(ui, BadgeKind::Warning, t("scan-vendor-warn-estimate"))
                .on_hover_text(&line.notes);
        }
        if line.error {
            widgets::badge(ui, BadgeKind::Warning, t("scan-vendor-warn-error"))
                .on_hover_text(&line.notes);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    /// What the MINI 2 reports when everything went well.
    fn good() -> VendorStatus {
        VendorStatus {
            fill_light: "on".into(),
            fill_light_note: None,
            fast_exposure: "applied".into(),
            fast_exposure_note: None,
            calibration: "factory",
            calibration_note: None,
            fx: 1142.37,
            fy: 1142.37,
            cx: 163.19,
            cy: 329.85,
        }
    }

    #[test]
    fn the_line_of_a_scanner_that_did_everything() {
        let l = VendorLine::of(&good());
        assert_eq!(
            l.text,
            "Light on · Fast exposure applied · Calibration: factory (fx 1142.4, cx 163.2, cy 329.9)"
        );
        assert!(l.notes.is_empty() && !l.error && !l.estimate);
    }

    #[test]
    fn every_state_has_its_words() {
        for (light, fast, cal) in [
            ("on", "applied", "user"),
            ("off", "pending", "factory-cached"),
            ("unsupported", "off", "estimate"),
            ("unsupported", "unsupported", "factory"),
        ] {
            let l = VendorLine::of(&VendorStatus {
                fill_light: light.into(),
                fast_exposure: fast.into(),
                calibration: cal,
                ..good()
            });
            assert!(
                !l.text.contains("scan-vendor"),
                "{light} {fast} {cal}: {}",
                l.text
            );
            assert!(!l.error);
        }
    }

    #[test]
    fn an_estimate_and_an_error_warn_and_the_notes_explain() {
        let l = VendorLine::of(&VendorStatus {
            fill_light: "error:device".into(),
            fill_light_note: Some("xu write failed".into()),
            calibration: "estimate",
            calibration_note: Some("no factory block".into()),
            ..good()
        });
        assert!(l.error && l.estimate);
        assert!(l.text.starts_with("Light not working ("), "{}", l.text);
        assert!(l.text.contains("Calibration: estimate"), "{}", l.text);
        assert!(l.notes.contains("xu write failed"), "{}", l.notes);
        assert!(l.notes.contains("no factory block"), "{}", l.notes);
        // a state of a newer core is shown, not hidden
        let l = VendorLine::of(&VendorStatus {
            fill_light: "dim".into(),
            ..good()
        });
        assert!(l.text.starts_with("Light: dim ·"), "{}", l.text);
        assert!(!l.error);
    }

    fn drawn_as(v: VendorStatus, advanced: bool) -> Harness<'static> {
        let mut h = Harness::builder()
            .with_context_setup(move |ctx| {
                dc_ui::theme::install(ctx, dc_ui::Mode::Dark);
                dc_module_api::set_advanced_ui(ctx, advanced);
            })
            .build_ui(move |ui| row(ui, &v));
        h.run();
        h
    }

    /// The row as the advanced view draws it.
    fn drawn(v: VendorStatus) -> Harness<'static> {
        drawn_as(v, true)
    }

    #[test]
    fn the_simple_view_keeps_the_warnings_and_hides_the_line() {
        let h = drawn_as(good(), false);
        assert!(h.query_by_label_contains("Calibration:").is_none());
        let h = drawn_as(
            VendorStatus {
                calibration: "estimate",
                fast_exposure: "error:device".into(),
                ..good()
            },
            false,
        );
        assert!(h.query_by_label_contains("Calibration:").is_none());
        assert!(
            h.query_by_label_contains("Approximate calibration")
                .is_some()
        );
        assert!(h.query_by_label_contains("did not apply").is_some());
    }

    #[test]
    fn the_row_shows_the_line_and_warns_only_when_it_should() {
        let h = drawn(good());
        assert!(
            h.query_by_label(
                "Light on · Fast exposure applied · Calibration: factory (fx 1142.4, cx 163.2, cy 329.9)"
            )
            .is_some()
        );
        assert!(
            h.query_by_label_contains("Approximate calibration")
                .is_none()
        );
        assert!(h.query_by_label_contains("did not apply").is_none());

        let h = drawn(VendorStatus {
            calibration: "estimate",
            ..good()
        });
        assert!(
            h.query_by_label_contains("Approximate calibration")
                .is_some()
        );
        assert!(h.query_by_label_contains("did not apply").is_none());

        let h = drawn(VendorStatus {
            fast_exposure: "error:device".into(),
            ..good()
        });
        assert!(h.query_by_label_contains("did not apply").is_some());
        assert!(
            h.query_by_label_contains("Approximate calibration")
                .is_none()
        );
    }
}
