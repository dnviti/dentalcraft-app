# dc-ui

DentalCraft design system for egui 0.37 ("curing light and shade guide",
design v0.8). Shared by `dc-app` and `dc-workspace`.

## API

```rust
use dc_ui::{theme, tokens, fonts, icons, widgets};

// once, at startup (fonts + visuals + spacing for both modes)
theme::install(ctx, theme::Mode::Dark);      // Mode::{Dark, Light}
theme::set_mode(ctx, theme::Mode::Light);    // switch later
let c = dc_ui::colors(ui);                   // &'static Palette of the mode `ui` paints with
let c = theme::ctx_colors(ctx);

// tokens (assets/tokens.json, parsed lazily)
let t = dc_ui::tokens();
t.colors(Mode::Dark).accent_amber;           // Color32; every colour token is a field
t.text.title_m.size; t.space.s4; t.radius.shade_tab; t.size.inspector_width;
t.shadow.e2_dialog;                          // theme::shadow(&t) -> egui::Shadow

// fonts
fonts::ui(13.0); fonts::medium(12.0); fonts::semibold(13.0); fonts::bold(..); fonts::extrabold(44.0);
fonts::mono(12.0); fonts::mono_bold(20.0); fonts::icon(16.0);
fonts::style(&t.text.display_hero);          // FontId of a type-scale token

// icons: Phosphor Regular codepoints, usable inside any text
format!("{} Save", icons::FLOPPY_DISK);

// widgets (all return egui::Response unless noted)
widgets::primary_button(ui, "New project");
widgets::secondary_button(ui, "Open project…");
widgets::ghost_button(ui, "Support");
widgets::Button::primary("New project").icon(icons::PLUS).large().enabled(ok).show(ui);
widgets::icon_button(ui, icons::GEAR, "Settings");          // tooltip = accessible label
widgets::badge(ui, widgets::BadgeKind::Success, "Signed in"); // Neutral/Info/Success/Warning/Danger/Amber
widgets::badge_icon(ui, BadgeKind::Info, Some(icons::STACK), "Staging");
widgets::status_dot(ui, c.status_success);
widgets::section_header(ui, "Modules", Some("1 installed, 4 on the way"));
widgets::card_frame(ui).show(ui, |ui| { .. });             // returns egui::Frame
widgets::sunken_frame(ui);                                  // returns egui::Frame
widgets::toggle(ui, &mut on, "Keep upper and lower in step");
widgets::checkbox(ui, &mut on, "Attach diagnostics bundle");
widgets::labeled_slider_mm(ui, "Translation", &mut v, 0.0..=1.0, 0.25); // also labeled_slider_deg / labeled_slider
widgets::tab_bar(ui, &mut idx, &["Stages", "Per tooth", "Report"]);   // .changed() on switch
widgets::segmented(ui, &mut idx, &["EN", "IT"], 28.0);  // segmented_fill: full width, equal items
widgets::text_input(ui, "Case ID", &mut s, "AL-2026-0142", Some(helper), error);
widgets::text_area(ui, "Notes", &mut s, "", 150.0);
widgets::progress_bar(ui, Some(0.4), 120.0);               // None = indeterminate
widgets::spinner(ui, 20.0); widgets::link(ui, "Read the release notes");
widgets::radio_indicator(ui, selected, enabled); widgets::divider_v(ui, 24.0); widgets::hairline(ui);
widgets::focus_ring(ui, &resp, rect, corner);              // for custom widgets
```

All interactive widgets are keyboard focusable, draw the 2 pt `focus_ring`
outside when focused and publish `WidgetInfo` (findable by label in
`egui_kittest`).

## i18n

Fluent bundle `i18n/{en,it}/dc-ui.ftl` (domain `dc-ui`), registered through
`dc_i18n::bundle!`; use `dc_ui::loader()` with `dc_i18n::fl!` or `dc_ui::tr(key)`.
Keys are prefixed `ui-`.

## Assets and licences

| Asset | Source | Licence |
|---|---|---|
| `assets/fonts/atkinson-hyperlegible-next/*.ttf` (Regular, Medium, SemiBold, Bold, ExtraBold) | github.com/googlefonts/atkinson-hyperlegible-next `fonts/ttf` | SIL OFL 1.1 (`OFL.txt`) |
| `assets/fonts/b612-mono/*.ttf` (Regular, Bold) | github.com/google/fonts `ofl/b612mono` | SIL OFL 1.1 (`OFL.txt`) |
| `assets/fonts/phosphor/Phosphor.ttf` | Phosphor Icons Regular (identical to the egui-phosphor 0.14 copy) | MIT (`LICENSE-Phosphor-MIT.txt`) |
| `src/phosphor_regular.rs` | codepoint list vendored from egui-phosphor 0.14.0 (no release supports egui 0.37) | MIT OR Apache-2.0 (`LICENSE-egui-phosphor-MIT.txt`) |
| `assets/tokens.json` | FigCraft export of `dentalcraft.figcraft` v0.8 (own work) | PolyForm Noncommercial 1.0.0 |
