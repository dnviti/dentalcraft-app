//! The resin form: the values of the resin maker for this printer, typed
//! once. Each field says what is wrong with it; nothing is saved until all
//! are right.

use dc_print::PrinterProfile;
use dc_ui::widgets;
use egui::Ui;

use crate::resin::Resin;
use crate::t;

/// The fields as typed.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ResinForm {
    pub name: String,
    pub layer_mm: String,
    pub exposure_s: String,
    pub bottom_exposure_s: String,
    pub bottom_layers: String,
    pub rest_s: String,
    pub density: String,
    /// Errors are shown once the user tried to save.
    pub tried: bool,
}

/// What the form answers.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FormAct {
    Save(Resin),
    Cancel,
}

/// A decimal number typed with a point or a comma.
fn number(s: &str) -> Option<f32> {
    s.trim()
        .replace(',', ".")
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
}

/// `v` without trailing zeros (`0.05`, `2.5`, `35`).
fn plain(v: f32) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

impl ResinForm {
    /// The form filled with `resin`, or empty.
    pub(crate) fn from_resin(resin: Option<&Resin>) -> Self {
        resin.map_or_else(Self::default, |r| Self {
            name: r.name.clone(),
            layer_mm: plain(r.layer_um as f32 / 1000.0),
            exposure_s: plain(r.exposure_s),
            bottom_exposure_s: plain(r.bottom_exposure_s),
            bottom_layers: r.bottom_layers.to_string(),
            rest_s: plain(r.rest_s),
            density: plain(r.density_g_ml),
            tried: false,
        })
    }

    fn name_error(&self) -> Option<String> {
        let n = self.name.trim();
        let ok =
            !n.is_empty() && n.len() < 32 && n.chars().all(|c| c.is_ascii_graphic() || c == ' ');
        (!ok).then(|| t("print-form-err-name"))
    }

    fn layer_error(&self, profile: &PrinterProfile) -> Option<String> {
        let ok = number(&self.layer_mm)
            .map(|mm| (mm * 1000.0).round() as u32)
            .is_some_and(|um| profile.supports_layer_um(um));
        (!ok).then(|| {
            dc_i18n::tr_args(
                "print-form-err-layer",
                &[
                    ("min", &plain(profile.layer_min_um as f32 / 1000.0)),
                    ("max", &plain(profile.layer_max_um as f32 / 1000.0)),
                ],
            )
        })
    }

    fn positive_error(s: &str) -> Option<String> {
        (!number(s).is_some_and(|v| v > 0.0)).then(|| t("print-form-err-positive"))
    }

    fn rest_error(&self) -> Option<String> {
        (!number(&self.rest_s).is_some_and(|v| v >= 0.0)).then(|| t("print-form-err-zero-or-more"))
    }

    fn count_error(&self) -> Option<String> {
        (self.bottom_layers.trim().parse::<u32>().is_err()).then(|| t("print-form-err-count"))
    }

    /// The resin, when every field is right.
    pub(crate) fn resin(&self, profile: &PrinterProfile) -> Option<Resin> {
        let errors = [
            self.name_error(),
            self.layer_error(profile),
            Self::positive_error(&self.exposure_s),
            Self::positive_error(&self.bottom_exposure_s),
            self.count_error(),
            self.rest_error(),
            Self::positive_error(&self.density),
        ];
        if errors.iter().any(Option::is_some) {
            return None;
        }
        Some(Resin {
            name: self.name.trim().to_owned(),
            layer_um: (number(&self.layer_mm)? * 1000.0).round() as u32,
            exposure_s: number(&self.exposure_s)?,
            bottom_exposure_s: number(&self.bottom_exposure_s)?,
            bottom_layers: self.bottom_layers.trim().parse().ok()?,
            rest_s: number(&self.rest_s)?,
            density_g_ml: number(&self.density)?,
        })
    }

    /// Draws the form; `can_cancel` when a resin is already saved.
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        profile: &PrinterProfile,
        can_cancel: bool,
    ) -> Option<FormAct> {
        crate::caption(ui, t("print-form-intro"));
        ui.add_space(8.0);
        let tried = self.tried;
        let err = |e: Option<String>| if tried { e } else { None };
        let name_e = err(self.name_error());
        widgets::text_input(
            ui,
            &t("print-form-name"),
            &mut self.name,
            &t("print-form-name-hint"),
            None,
            name_e.as_deref(),
        );
        ui.add_space(4.0);
        // the numbers two by two, so the whole form fits the column
        let errors = [
            err(self.layer_error(profile)),
            err(Self::positive_error(&self.exposure_s)),
            err(Self::positive_error(&self.bottom_exposure_s)),
            err(self.count_error()),
            err(self.rest_error()),
            err(Self::positive_error(&self.density)),
        ];
        let fields: [(&str, &mut String, &str, Option<&str>); 6] = [
            ("print-form-layer", &mut self.layer_mm, "0.05", None),
            ("print-form-exposure", &mut self.exposure_s, "2.5", None),
            (
                "print-form-bottom-exposure",
                &mut self.bottom_exposure_s,
                "30",
                Some("print-form-bottom-exposure-help"),
            ),
            (
                "print-form-bottom-layers",
                &mut self.bottom_layers,
                "4",
                None,
            ),
            (
                "print-form-rest",
                &mut self.rest_s,
                "1",
                Some("print-form-rest-help"),
            ),
            ("print-form-density", &mut self.density, "1.1", None),
        ];
        let mut fields = fields.into_iter().zip(errors);
        while let (Some(a), b) = (fields.next(), fields.next()) {
            ui.columns(2, |cols| {
                for (col, ((key, value, hint, help), e)) in
                    cols.iter_mut().zip([Some(a), b].into_iter().flatten())
                {
                    let r = widgets::text_input(col, &t(key), value, hint, None, e.as_deref());
                    if let Some(help) = help {
                        r.on_hover_text(t(help));
                    }
                }
            });
            ui.add_space(4.0);
        }
        drop(fields);
        ui.add_space(10.0);
        let mut act = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if widgets::Button::primary(t("print-form-save"))
                .show(ui)
                .clicked()
            {
                self.tried = true;
                if let Some(r) = self.resin(profile) {
                    act = Some(FormAct::Save(r));
                }
            }
            if can_cancel
                && widgets::Button::secondary(t("print-form-cancel"))
                    .show(ui)
                    .clicked()
            {
                act = Some(FormAct::Cancel);
            }
        });
        act
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled() -> ResinForm {
        ResinForm {
            name: "Clear aligner".into(),
            layer_mm: "0,1".into(),
            exposure_s: "5.7".into(),
            bottom_exposure_s: "35".into(),
            bottom_layers: "4".into(),
            rest_s: "0".into(),
            density: "1.1".into(),
            tried: true,
        }
    }

    #[test]
    fn a_filled_form_gives_the_resin_and_back() {
        let p = PrinterProfile::saturn_4_ultra_16k();
        let r = filled().resin(&p).unwrap();
        assert_eq!(r.layer_um, 100);
        assert_eq!(r.bottom_layers, 4);
        assert_eq!(r.rest_s, 0.0);
        let again = ResinForm::from_resin(Some(&r));
        assert_eq!(again.layer_mm, "0.1");
        assert_eq!(again.resin(&p), Some(r));
    }

    #[test]
    fn a_wrong_value_gives_no_resin() {
        let p = PrinterProfile::saturn_4_ultra_16k();
        for f in [
            ResinForm {
                layer_mm: "0.5".into(),
                ..filled()
            },
            ResinForm {
                exposure_s: "0".into(),
                ..filled()
            },
            ResinForm {
                name: "Résine".into(),
                ..filled()
            },
            ResinForm {
                bottom_layers: "4.5".into(),
                ..filled()
            },
            ResinForm {
                rest_s: "-1".into(),
                ..filled()
            },
            ResinForm::default(),
        ] {
            assert_eq!(f.resin(&p), None, "{f:?}");
        }
    }
}
