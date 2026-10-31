//! Drawn illustrations and small painted elements of the shell: logo tile,
//! arch thumbnail, shade guide, dashed shade-tab outlines, numbered steps,
//! avatar.

use std::f32::consts::PI;

use dc_ui::{Palette, fonts, icons, widgets::shade_tab_radius};
use egui::epaint::{Mesh, Vertex};
use egui::{
    Align2, Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
    pos2, vec2,
};

/// Amber logo tile (shade-tab 10/10/3/3) with the tooth glyph.
pub fn logo_tile(ui: &mut Ui, c: &Palette, size: Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, shade_tab_radius(10.0, 3.0), c.accent_amber);
    p.text(
        rect.center() + vec2(0.0, 1.0),
        Align2::CENTER_CENTER,
        icons::TOOTH,
        fonts::icon((size.y * 0.56).round()),
        c.accent_amber_on,
    );
    resp
}

/// Rounded-rectangle outline as a closed point list (`r` = nw, ne, se, sw).
fn rounded_outline(rect: Rect, r: [f32; 4]) -> Vec<Pos2> {
    let mut pts = Vec::new();
    let corners = [
        (pos2(rect.left() + r[0], rect.top() + r[0]), r[0], PI),
        (pos2(rect.right() - r[1], rect.top() + r[1]), r[1], 1.5 * PI),
        (pos2(rect.right() - r[2], rect.bottom() - r[2]), r[2], 0.0),
        (
            pos2(rect.left() + r[3], rect.bottom() - r[3]),
            r[3],
            0.5 * PI,
        ),
    ];
    for (center, radius, start) in corners {
        for i in 0..=8 {
            let a = start + i as f32 / 8.0 * 0.5 * PI;
            pts.push(center + radius * vec2(a.cos(), a.sin()));
        }
    }
    pts.push(pts[0]);
    pts
}

/// Dashed outline (4/4) of a rounded rectangle: coming-soon cards and drop zones.
pub fn dashed_rect(p: &Painter, rect: Rect, r: [f32; 4], stroke: Stroke) {
    let pts = rounded_outline(rect, r);
    p.extend(Shape::dashed_line(&pts, stroke, 4.0, 4.0));
}

/// Numbered step marker (24 pt blue disc), as "1 Module".
pub fn step_number(ui: &mut Ui, c: &Palette, n: u32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 12.0, c.select_blue);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        n.to_string(),
        fonts::bold(12.0),
        c.bg_canvas,
    );
    resp
}

/// Initials of a display name (`Giulia Rossi` -> `GR`).
pub fn initials(name: &str) -> String {
    let mut s: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    if s.is_empty() {
        s.push('?');
    }
    s
}

/// 32 pt avatar: curing-blue ring with the initials.
pub fn avatar(p: &Painter, c: &Palette, center: Pos2, name: &str) {
    p.circle(center, 15.0, c.select_soft, Stroke::new(2.0, c.select_blue));
    p.text(
        center,
        Align2::CENTER_CENTER,
        initials(name),
        fonts::bold(11.0),
        c.select_blue,
    );
}

/// Shade-tab code tile (as `AL` in the module chooser).
pub fn code_tile(ui: &mut Ui, c: &Palette, code: &str, size: Vec2, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, shade_tab_radius(8.0, 3.0), c.bg_sunken);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        code,
        fonts::mono_bold(16.0),
        if active {
            c.accent_amber_text
        } else {
            c.text_secondary
        },
    );
    resp
}

fn arch_point(center: Pos2, a: f32, b: f32, t: f32) -> Pos2 {
    // t in 0..=1 from the left end to the right end; posterior teeth flare out
    let th = t * PI;
    let flare = 1.0 + 0.12 * (1.0 - th.sin());
    pos2(center.x - a * th.cos() * flare, center.y - b * th.sin())
}

/// Occlusal-view dental arch drawn into `rect` (gingiva band, palate and
/// 14 teeth), used when a project has no thumbnail and as scan preview.
pub fn arch(p: &Painter, c: &Palette, rect: Rect) {
    let h = rect.height().min(rect.width() * 0.8);
    let a = h * 0.5;
    let b = h * 0.86;
    let center = pos2(rect.center().x, rect.center().y + b * 0.5);
    let band = h * 0.13;
    let n = 48;
    let outer: Vec<Pos2> = (0..=n)
        .map(|i| arch_point(center, a + band, b + band, i as f32 / n as f32))
        .collect();
    let inner: Vec<Pos2> = (0..=n)
        .map(|i| arch_point(center, a - band, b - band, i as f32 / n as f32))
        .collect();
    // palate
    p.add(Shape::convex_polygon(
        inner.clone(),
        mix(c.model_gingiva, c.bg_viewport, 0.55),
        Stroke::NONE,
    ));
    // gingiva band as a triangle strip
    let mut mesh = Mesh::default();
    for (o, i) in outer.iter().zip(&inner) {
        mesh.vertices.push(Vertex {
            pos: *o,
            uv: egui::epaint::WHITE_UV,
            color: c.model_gingiva,
        });
        mesh.vertices.push(Vertex {
            pos: *i,
            uv: egui::epaint::WHITE_UV,
            color: c.model_gingiva,
        });
    }
    for k in 0..n as u32 {
        let i = 2 * k;
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i + 1, i + 3, i + 2);
    }
    p.add(Shape::mesh(mesh));
    // teeth
    let teeth = 14;
    for k in 0..teeth {
        let t = (k as f32 + 0.5) / teeth as f32;
        let center_t = arch_point(center, a, b, t);
        let post = 1.0 - (t * PI).sin();
        let r = band * (0.62 + 0.45 * post);
        p.circle(
            center_t,
            r,
            c.model_enamel,
            Stroke::new(1.5, c.model_enamel_shade),
        );
    }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    dc_ui::theme::mix(a, b, t)
}

fn gradient_rect(p: &Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = Mesh::default();
    let v = |pos: Pos2, color: Color32| Vertex {
        pos,
        uv: egui::epaint::WHITE_UV,
        color,
    };
    mesh.vertices.extend([
        v(rect.left_top(), top),
        v(rect.right_top(), top),
        v(rect.right_bottom(), bottom),
        v(rect.left_bottom(), bottom),
    ]);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(Shape::mesh(mesh));
}

/// The shade-guide illustration of the sign-in story panel: five tabs
/// (A1, A2, A3, B2, C1), A3 highlighted in amber, bottoms aligned on
/// `rect.bottom()`, centred-left in `rect`.
pub fn shade_guide(p: &Painter, c: &Palette, rect: Rect) {
    let tabs: [(&str, f32, [u8; 3], [u8; 3]); 5] = [
        ("A1", 300.0, [0xF2, 0xEB, 0xDD], [0xE6, 0xDC, 0xC8]),
        ("A2", 360.0, [0xEF, 0xE3, 0xCC], [0xE0, 0xD0, 0xB2]),
        ("A3", 420.0, [0xE8, 0xD3, 0xAD], [0xD6, 0xB9, 0x8C]),
        ("B2", 360.0, [0xED, 0xE3, 0xCB], [0xDE, 0xD0, 0xB0]),
        ("C1", 300.0, [0xDD, 0xD5, 0xC3], [0xC9, 0xBE, 0xA8]),
    ];
    let (w, gap) = (92.0, 18.0);
    let scale = (rect.height() / 420.0)
        .min(rect.width() / (5.0 * w + 4.0 * gap))
        .min(1.0);
    let (w, gap) = (w * scale, gap * scale);
    let handle = 40.0 * scale;
    let mut x = rect.left();
    for (code, height, top, bottom) in tabs {
        let h = height * scale;
        let r = Rect::from_min_size(pos2(x, rect.bottom() - h), vec2(w, h));
        x += w + gap;
        let highlight = code == "A3";
        let body = Rect::from_min_max(pos2(r.left(), r.top() + handle), r.max);
        gradient_rect(
            p,
            body,
            Color32::from_rgb(top[0], top[1], top[2]),
            Color32::from_rgb(bottom[0], bottom[1], bottom[2]),
        );
        let band = Rect::from_min_size(r.min, vec2(w, handle));
        let (band_fill, text) = if highlight {
            (c.accent_amber, c.accent_amber_on)
        } else {
            (
                Color32::from_rgb(0x2A, 0x31, 0x36),
                Color32::from_rgb(0xEC, 0xE6, 0xDA),
            )
        };
        p.rect_filled(band, shade_tab_radius(16.0 * scale, 0.0), band_fill);
        p.text(
            band.center(),
            Align2::CENTER_CENTER,
            code,
            fonts::mono_bold((14.0 * scale).max(9.0)),
            text,
        );
        if highlight {
            p.rect_stroke(
                r,
                shade_tab_radius(16.0 * scale, 4.0),
                Stroke::new(1.5, c.accent_amber),
                StrokeKind::Outside,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_and_outline() {
        assert_eq!(initials("Giulia Rossi"), "GR");
        assert_eq!(initials("demo"), "D");
        assert_eq!(initials(""), "?");
        let pts = rounded_outline(
            Rect::from_min_size(Pos2::ZERO, vec2(100.0, 50.0)),
            [16.0, 16.0, 4.0, 4.0],
        );
        assert_eq!(pts.first(), pts.last());
        assert!(pts.iter().all(|p| p.x >= -0.01 && p.x <= 100.01));
    }
}
