//! DentalCraft design system for egui ("curing light and shade guide").
//!
//! - [`tokens()`]: typed tokens from `assets/tokens.json` (colours per mode,
//!   type scale, spacing, radii, strokes, shadows, component sizes).
//! - [`theme::install`]: fonts + visuals + spacing for both modes.
//! - [`fonts`]: `FontId` helpers (Atkinson Hyperlegible Next weights, B612
//!   Mono, Phosphor icons).
//! - [`icons`]: Phosphor Regular codepoints (`icons::TOOTH`, ...), usable in
//!   any text because Phosphor is a fallback of every family.
//! - [`widgets`]: buttons, badges, toggles, sliders, tabs, inputs, cards.
//! - [`panels`]: side panels the user resizes, contents that never move a
//!   panel's edge.
//! - [`keys`]: key names of the platform in shortcut texts (`Ctrl` is `Cmd` on macOS).
//! - [`pacing`]: the frame rate cap (60 fps).
//! - [`loader`] / [`tr`]: the Fluent bundle of shared widget strings.
//!
//! ```no_run
//! # fn f(ctx: &egui::Context, ui: &mut egui::Ui) {
//! dc_ui::theme::install(ctx, dc_ui::theme::Mode::Dark);
//! if dc_ui::widgets::Button::primary("New project")
//!     .icon(dc_ui::icons::PLUS)
//!     .large()
//!     .show(ui)
//!     .clicked()
//! { /* ... */ }
//! # }
//! ```

pub mod fonts;
#[path = "phosphor_regular.rs"]
pub mod icons;
pub mod keys;
pub mod pacing;
pub mod panels;
pub mod theme;
pub mod tokens;
pub mod widgets;

pub use theme::{Mode, colors};
pub use tokens::{Palette, Tokens, tokens};

dc_i18n::bundle!(pub fn loader);

/// Translates a key of this crate's bundle (or any registered bundle).
pub fn tr(key: &str) -> String {
    loader();
    dc_i18n::tr(key)
}

#[cfg(test)]
mod tests {
    #[test]
    fn translations_are_complete() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("i18n");
        dc_i18n::parity::check_dir(&dir).unwrap();
    }

    #[test]
    fn bundle_resolves() {
        assert_eq!(dc_i18n::fl!(super::loader(), "ui-theme-dark"), "Dark");
    }
}
