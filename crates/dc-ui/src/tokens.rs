//! Typed design tokens, parsed once from `assets/tokens.json` (the FigCraft
//! export of the DentalCraft design system, v0.8).

use std::collections::BTreeMap;
use std::sync::LazyLock;

use egui::Color32;
use serde::Deserialize;

use crate::theme::Mode;

/// The raw `tokens.json` embedded in the binary.
pub const TOKENS_JSON: &str = include_str!("../assets/tokens.json");

static TOKENS: LazyLock<Tokens> =
    LazyLock::new(|| Tokens::from_json(TOKENS_JSON).expect("assets/tokens.json is valid"));

/// The design tokens, parsed on first use.
pub fn tokens() -> &'static Tokens {
    &TOKENS
}

#[derive(Deserialize)]
struct RawColor {
    rgba: [u8; 4],
}

macro_rules! palette {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {
        /// Colours of one theme mode (`color.dark` or `color.light`).
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub struct Palette {
            $($(#[$doc])* pub $name: Color32,)*
        }

        impl Palette {
            fn from_map(map: &BTreeMap<String, RawColor>) -> Result<Self, String> {
                let get = |k: &str| {
                    map.get(k)
                        .map(|c| {
                            let [r, g, b, a] = c.rgba;
                            Color32::from_rgba_unmultiplied(r, g, b, a)
                        })
                        .ok_or_else(|| format!("missing colour token {k}"))
                };
                Ok(Self { $($name: get(stringify!($name))?,)* })
            }

            /// Colour by token name (`bg_panel`, `select_blue`, ...).
            pub fn get(&self, name: &str) -> Option<Color32> {
                match name {
                    $(stringify!($name) => Some(self.$name),)*
                    _ => None,
                }
            }
        }
    };
}

palette! {
    /// Window background behind panels.
    bg_canvas,
    /// Panels, top bar, cards on Home.
    bg_panel,
    /// Raised surfaces: secondary buttons, popovers, dialogs.
    bg_raised,
    /// Sunken surfaces: inputs, shade-tab handles, licence blocks.
    bg_sunken,
    /// Hover fill.
    bg_hover,
    /// 3D viewport and project thumbnails (edges).
    bg_viewport,
    /// 3D viewport (centre of the radial gradient).
    bg_viewport_center,
    /// Hairlines between regions.
    border_subtle,
    /// Input and secondary button borders (3:1).
    border_strong,
    /// Main text.
    text_primary,
    /// Secondary text.
    text_secondary,
    /// Disabled text.
    text_disabled,
    /// Action colour (primary buttons, links).
    accent_amber,
    /// Primary button hover.
    accent_amber_hover,
    /// Primary button pressed.
    accent_amber_pressed,
    /// Text on amber.
    accent_amber_on,
    /// Amber used as text colour.
    accent_amber_text,
    /// Amber tint.
    accent_amber_soft,
    /// Selection, current stage.
    select_blue,
    /// Selection tint.
    select_soft,
    /// Focus ring.
    focus_ring,
    /// Success.
    status_success,
    /// Warning.
    status_warning,
    /// Danger.
    status_danger,
    /// Tooth enamel.
    model_enamel,
    /// Enamel outline.
    model_enamel_shade,
    /// Gingiva.
    model_gingiva,
    /// Attachments.
    model_attachment,
    /// IPR planes.
    model_ipr,
    /// Margin line of a tooth region on the scan.
    model_margin,
}

/// Font family of a text style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FamilyToken {
    /// Atkinson Hyperlegible Next.
    Ui,
    /// B612 Mono.
    Mono,
    /// Phosphor icons.
    Icons,
}

/// Font weight of a text style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Weight {
    /// 400.
    #[default]
    Regular,
    /// 500.
    Medium,
    /// 600.
    Semibold,
    /// 700.
    Bold,
    /// 800.
    Extrabold,
}

/// One entry of `text_style`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct TextStyleToken {
    /// Family.
    pub family: FamilyToken,
    /// Weight (Regular when absent).
    #[serde(default)]
    pub weight: Weight,
    /// Size in points.
    pub size: f32,
    /// Line height in points.
    #[serde(default)]
    pub line_height: Option<f32>,
}

/// The type scale.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[allow(missing_docs)]
pub struct TextStyles {
    pub display_hero: TextStyleToken,
    pub display_l: TextStyleToken,
    pub title_m: TextStyleToken,
    pub title_s: TextStyleToken,
    pub body_l: TextStyleToken,
    pub body_m: TextStyleToken,
    pub body_m_strong: TextStyleToken,
    pub label_s: TextStyleToken,
    pub caption: TextStyleToken,
    pub mono_readout: TextStyleToken,
    pub mono_readout_l: TextStyleToken,
    pub icon_16: TextStyleToken,
    pub icon_20: TextStyleToken,
    pub icon_32: TextStyleToken,
}

/// Spacing scale (4 pt grid).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[allow(missing_docs)]
pub struct Space {
    pub s0: f32,
    pub s1: f32,
    pub s2: f32,
    pub s3: f32,
    pub s4: f32,
    pub s6: f32,
    pub s8: f32,
    pub s12: f32,
}

/// Corner radii.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Radius {
    /// 2.
    pub xs: f32,
    /// 4: inputs, buttons.
    pub sm: f32,
    /// 8: panels, project cards.
    pub md: f32,
    /// 12: cards, dialogs.
    pub lg: f32,
    /// Shade-tab shape: nw, ne, sw, se (16/16/4/4).
    pub shade_tab: [f32; 4],
    /// Pill.
    pub pill: f32,
}

/// Stroke widths.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[allow(missing_docs)]
pub struct Strokes {
    pub hairline: f32,
    pub focus_ring: f32,
    pub active_bar: f32,
    pub selected_tooth: f32,
}

/// A drop shadow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowToken {
    /// Offset x, y.
    pub offset: [f32; 2],
    /// Blur radius.
    pub blur: f32,
    /// Spread.
    pub spread: f32,
    /// Colour.
    pub color: Color32,
}

#[derive(Deserialize)]
struct RawShadow {
    offset: [f32; 2],
    blur: f32,
    spread: f32,
    color: String,
}

/// Elevations (e0 is flat, so only e1 and e2 exist).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadows {
    /// Popovers and things floating over the 3D view.
    pub e1_popover: ShadowToken,
    /// Dialogs.
    pub e2_dialog: ShadowToken,
}

/// Fixed component and layout sizes.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[allow(missing_docs)]
pub struct Sizes {
    pub topbar_home: f32,
    pub topbar_workspace: f32,
    pub statusbar: f32,
    pub left_menu_width: f32,
    pub inspector_width: f32,
    pub timeline_height: f32,
    pub button_h: f32,
    pub button_h_large: f32,
    pub button_h_cta: f32,
    pub input_h: f32,
    pub menu_group_row: f32,
    pub menu_item_row: f32,
    pub menu_sub_row: f32,
    pub menu_indent: [f32; 3],
    pub icon_button: f32,
    pub min_hit_target: f32,
    pub module_card_width: f32,
    pub project_card_width: f32,
}

/// Token file metadata.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Meta {
    /// Design system name.
    pub name: String,
    /// Design system version.
    pub version: String,
}

/// All design tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct Tokens {
    /// Metadata.
    pub meta: Meta,
    /// Dark mode colours (default).
    pub dark: Palette,
    /// Light mode colours.
    pub light: Palette,
    /// Type scale.
    pub text: TextStyles,
    /// Spacing.
    pub space: Space,
    /// Radii.
    pub radius: Radius,
    /// Stroke widths.
    pub stroke: Strokes,
    /// Elevations.
    pub shadow: Shadows,
    /// Component sizes.
    pub size: Sizes,
}

#[derive(Deserialize)]
struct RawTokens {
    meta: Meta,
    color: BTreeMap<String, BTreeMap<String, RawColor>>,
    text_style: TextStyles,
    space: Space,
    radius: Radius,
    stroke: Strokes,
    shadow: BTreeMap<String, Option<RawShadow>>,
    size: Sizes,
}

/// Parses `#RRGGBB` or `#RRGGBBAA` (straight alpha).
pub fn parse_hex(s: &str) -> Option<Color32> {
    let h = s.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        6 => Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(
            byte(0)?,
            byte(2)?,
            byte(4)?,
            byte(6)?,
        )),
        _ => None,
    }
}

impl Tokens {
    /// Parses a token file with the layout of `assets/tokens.json`.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let raw: RawTokens = serde_json::from_str(json).map_err(|e| e.to_string())?;
        // the JSON calls it `accent_on_amber`; the struct groups it with the other ambers
        let mode = |name: &str| -> Result<Palette, String> {
            let mut map = raw
                .color
                .get(name)
                .ok_or_else(|| format!("missing color.{name}"))?
                .iter()
                .map(|(k, v)| (k.clone(), RawColor { rgba: v.rgba }))
                .collect::<BTreeMap<_, _>>();
            if let Some(c) = map.remove("accent_on_amber") {
                map.insert("accent_amber_on".into(), c);
            }
            Palette::from_map(&map)
        };
        let shadow = |name: &str| -> Result<ShadowToken, String> {
            let s = raw
                .shadow
                .get(name)
                .and_then(Option::as_ref)
                .ok_or_else(|| format!("missing shadow.{name}"))?;
            Ok(ShadowToken {
                offset: s.offset,
                blur: s.blur,
                spread: s.spread,
                color: parse_hex(&s.color).ok_or_else(|| format!("bad colour {}", s.color))?,
            })
        };
        Ok(Self {
            dark: mode("dark")?,
            light: mode("light")?,
            shadow: Shadows {
                e1_popover: shadow("e1_popover")?,
                e2_dialog: shadow("e2_dialog")?,
            },
            meta: raw.meta,
            text: raw.text_style,
            space: raw.space,
            radius: raw.radius,
            stroke: raw.stroke,
            size: raw.size,
        })
    }

    /// The palette of `mode`.
    pub fn colors(&self, mode: Mode) -> &Palette {
        match mode {
            Mode::Dark => &self.dark,
            Mode::Light => &self.light,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_tokens_parse() {
        let t = tokens();
        assert_eq!(t.meta.name, "DentalCraft");
        assert_eq!(t.dark.bg_panel, Color32::from_rgb(0x16, 0x1B, 0x1E));
        assert_eq!(
            t.light.accent_amber_text,
            Color32::from_rgb(0x8A, 0x52, 0x00)
        );
        assert_eq!(t.dark.accent_amber_on, Color32::from_rgb(0x1C, 0x12, 0x04));
        assert_eq!(t.dark.select_soft.a(), 41);
        assert_eq!(t.text.display_hero.size, 44.0);
        assert_eq!(t.text.display_hero.weight, Weight::Extrabold);
        assert_eq!(t.text.mono_readout.family, FamilyToken::Mono);
        assert_eq!(t.radius.shade_tab, [16.0, 16.0, 4.0, 4.0]);
        assert_eq!(t.size.left_menu_width, 288.0);
        assert_eq!(t.shadow.e2_dialog.blur, 32.0);
        assert_eq!(t.dark.get("select_blue"), Some(t.dark.select_blue));
        assert_eq!(t.colors(Mode::Light), &t.light);
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#0F1214"), Some(Color32::from_rgb(15, 18, 20)));
        assert_eq!(parse_hex("#00000059").map(|c| c.a()), Some(0x59));
        assert_eq!(parse_hex("0F1214"), None);
        assert_eq!(parse_hex("#0F12"), None);
    }
}
