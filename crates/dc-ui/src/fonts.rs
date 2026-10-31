//! Fonts: Atkinson Hyperlegible Next (UI text, five weights), B612 Mono
//! (readouts) and Phosphor Regular (icons), all embedded.
//!
//! egui has no weight axis, so every weight is its own
//! [`FontFamily::Name`]. Phosphor is a fallback of every family, so icon
//! constants from [`crate::icons`] can be mixed into any text.

use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily, FontId};

use crate::tokens::{FamilyToken, TextStyleToken, Weight};

const ATK_REGULAR: &[u8] = include_bytes!(
    "../assets/fonts/atkinson-hyperlegible-next/AtkinsonHyperlegibleNext-Regular.ttf"
);
const ATK_MEDIUM: &[u8] = include_bytes!(
    "../assets/fonts/atkinson-hyperlegible-next/AtkinsonHyperlegibleNext-Medium.ttf"
);
const ATK_SEMIBOLD: &[u8] = include_bytes!(
    "../assets/fonts/atkinson-hyperlegible-next/AtkinsonHyperlegibleNext-SemiBold.ttf"
);
const ATK_BOLD: &[u8] =
    include_bytes!("../assets/fonts/atkinson-hyperlegible-next/AtkinsonHyperlegibleNext-Bold.ttf");
const ATK_EXTRABOLD: &[u8] = include_bytes!(
    "../assets/fonts/atkinson-hyperlegible-next/AtkinsonHyperlegibleNext-ExtraBold.ttf"
);
const B612_REGULAR: &[u8] = include_bytes!("../assets/fonts/b612-mono/B612Mono-Regular.ttf");
const B612_BOLD: &[u8] = include_bytes!("../assets/fonts/b612-mono/B612Mono-Bold.ttf");
const PHOSPHOR: &[u8] = include_bytes!("../assets/fonts/phosphor/Phosphor.ttf");

/// Family name of the Medium weight.
pub const MEDIUM: &str = "atkinson-medium";
/// Family name of the SemiBold weight.
pub const SEMIBOLD: &str = "atkinson-semibold";
/// Family name of the Bold weight.
pub const BOLD: &str = "atkinson-bold";
/// Family name of the ExtraBold weight.
pub const EXTRABOLD: &str = "atkinson-extrabold";
/// Family name of B612 Mono Bold.
pub const MONO_BOLD: &str = "b612-mono-bold";
/// Family name of the icon font alone (Phosphor first).
pub const ICONS: &str = "phosphor";

/// Font definitions with the DentalCraft families; egui's default fonts stay
/// as last fallbacks (emoji, symbols).
pub fn definitions() -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let fallbacks_prop = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let fallbacks_mono = defs
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();

    let data = [
        ("atkinson-regular", ATK_REGULAR),
        (MEDIUM, ATK_MEDIUM),
        (SEMIBOLD, ATK_SEMIBOLD),
        (BOLD, ATK_BOLD),
        (EXTRABOLD, ATK_EXTRABOLD),
        ("b612-mono-regular", B612_REGULAR),
        (MONO_BOLD, B612_BOLD),
        (ICONS, PHOSPHOR),
    ];
    for (name, bytes) in data {
        defs.font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }

    // Phosphor maps a-z to blank ligature glyphs: it must come after a text font.
    let chain = |first: &str, rest: &[String]| -> Vec<String> {
        let mut v = vec![first.to_owned(), ICONS.to_owned()];
        v.extend(rest.iter().cloned());
        v
    };
    defs.families.insert(
        FontFamily::Proportional,
        chain("atkinson-regular", &fallbacks_prop),
    );
    defs.families.insert(
        FontFamily::Monospace,
        chain("b612-mono-regular", &fallbacks_mono),
    );
    for name in [MEDIUM, SEMIBOLD, BOLD, EXTRABOLD] {
        defs.families
            .insert(FontFamily::Name(name.into()), chain(name, &fallbacks_prop));
    }
    defs.families.insert(
        FontFamily::Name(MONO_BOLD.into()),
        chain(MONO_BOLD, &fallbacks_mono),
    );
    let mut icons = vec![ICONS.to_owned()];
    icons.extend(fallbacks_prop.iter().cloned());
    defs.families.insert(FontFamily::Name(ICONS.into()), icons);
    defs
}

/// Installs [`definitions`] on the context.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}

/// The egui family of an Atkinson weight.
pub fn family(weight: Weight) -> FontFamily {
    match weight {
        Weight::Regular => FontFamily::Proportional,
        Weight::Medium => FontFamily::Name(MEDIUM.into()),
        Weight::Semibold => FontFamily::Name(SEMIBOLD.into()),
        Weight::Bold => FontFamily::Name(BOLD.into()),
        Weight::Extrabold => FontFamily::Name(EXTRABOLD.into()),
    }
}

/// UI text, Regular.
pub fn ui(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// UI text with a weight.
pub fn ui_weight(size: f32, weight: Weight) -> FontId {
    FontId::new(size, family(weight))
}

/// UI text, Medium.
pub fn medium(size: f32) -> FontId {
    ui_weight(size, Weight::Medium)
}

/// UI text, SemiBold.
pub fn semibold(size: f32) -> FontId {
    ui_weight(size, Weight::Semibold)
}

/// UI text, Bold.
pub fn bold(size: f32) -> FontId {
    ui_weight(size, Weight::Bold)
}

/// UI text, ExtraBold.
pub fn extrabold(size: f32) -> FontId {
    ui_weight(size, Weight::Extrabold)
}

/// Readouts (B612 Mono Regular).
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Readouts, bold (B612 Mono Bold).
pub fn mono_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MONO_BOLD.into()))
}

/// Icons only (Phosphor first in the chain).
pub fn icon(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ICONS.into()))
}

/// The font of a token text style (e.g. `tokens().text.title_m`).
pub fn style(t: &TextStyleToken) -> FontId {
    match t.family {
        FamilyToken::Ui => ui_weight(t.size, t.weight),
        FamilyToken::Mono if t.weight >= Weight::Bold => mono_bold(t.size),
        FamilyToken::Mono => mono(t.size),
        FamilyToken::Icons => icon(t.size),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_are_defined_with_icon_fallback() {
        let defs = definitions();
        for fam in [
            FontFamily::Proportional,
            FontFamily::Monospace,
            family(Weight::Extrabold),
            FontFamily::Name(MONO_BOLD.into()),
        ] {
            let chain = &defs.families[&fam];
            assert_eq!(chain[1], ICONS, "{fam:?}");
            assert!(defs.font_data.contains_key(&chain[0]));
        }
        assert_eq!(style(&crate::tokens().text.mono_readout_l), mono_bold(20.0));
        assert_eq!(style(&crate::tokens().text.body_m), ui(13.0));
    }
}
