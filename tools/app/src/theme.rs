// Colours, fonts and the small set of widgets the app is drawn from. The palette is the one the
// HTML version used (its CSS custom properties), light and dark.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Response, RichText, Sense, Shadow, Stroke, Ui, Vec2,
};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct Pal {
    pub dark: bool,
    pub shell: Color32,
    pub card: Color32,
    pub line: Color32,
    pub ink: Color32,
    pub ink2: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub accent_ink: Color32,
    pub accent_fill: Color32,
    pub chip: Color32,
    pub row: Color32,
    pub track: Color32,
    pub btn: Color32,
    pub grid: Color32,
    pub grid_zero: Color32,
    pub band_line: Color32,
    pub node_dim: Color32,
    pub node_dim_ink: Color32,
    pub pos: Color32,
    pub warn: Color32,
    pub ok_bg: Color32,
    pub warn_bg: Color32,
    pub ok_dot: Color32,
    pub danger: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub const LIGHT: Pal = Pal {
    dark: false,
    shell: rgb(0xf1, 0xf2, 0xf4),
    card: rgb(0xff, 0xff, 0xff),
    line: rgb(0xe4, 0xe4, 0xe7),
    ink: rgb(0x18, 0x18, 0x1b),
    ink2: rgb(0x3f, 0x3f, 0x46),
    muted: rgb(0x71, 0x71, 0x7a),
    faint: rgb(0xa1, 0xa1, 0xaa),
    accent: rgb(57, 83, 168),
    accent_ink: rgb(0xff, 0xff, 0xff),
    accent_fill: Color32::from_rgba_premultiplied(10, 15, 30, 46),
    chip: rgb(0xf4, 0xf4, 0xf5),
    row: rgb(0xfa, 0xfa, 0xfa),
    track: rgb(0xee, 0xee, 0xf0),
    btn: rgb(0xff, 0xff, 0xff),
    grid: rgb(0xf0, 0xf0, 0xf2),
    grid_zero: rgb(0xd4, 0xd4, 0xd8),
    band_line: rgb(0xdc, 0xdc, 0xe0),
    node_dim: rgb(0xe4, 0xe4, 0xe7),
    node_dim_ink: rgb(0xa1, 0xa1, 0xaa),
    pos: rgb(0, 129, 48),
    warn: rgb(191, 89, 0),
    ok_bg: rgb(0xf3, 0xf7, 0xfa),
    warn_bg: rgb(0xfd, 0xf6, 0xec),
    ok_dot: rgb(0, 162, 69),
    danger: rgb(0xef, 0x44, 0x44),
};

pub const DARK: Pal = Pal {
    dark: true,
    shell: rgb(0x10, 0x12, 0x16),
    card: rgb(0x1a, 0x1d, 0x23),
    line: rgb(0x27, 0x2b, 0x33),
    ink: rgb(0xe6, 0xe9, 0xee),
    ink2: rgb(0xc6, 0xcc, 0xd6),
    muted: rgb(0x8e, 0x96, 0xa3),
    faint: rgb(0x6b, 0x74, 0x80),
    accent: rgb(106, 159, 238),
    accent_ink: rgb(0x0c, 0x0e, 0x12),
    accent_fill: Color32::from_rgba_premultiplied(19, 29, 43, 46),
    chip: rgb(0x1f, 0x23, 0x2a),
    row: rgb(0x16, 0x19, 0x1f),
    track: rgb(0x27, 0x2b, 0x33),
    btn: rgb(0x1a, 0x1d, 0x23),
    grid: rgb(0x1b, 0x1f, 0x26),
    grid_zero: rgb(0x3a, 0x3f, 0x47),
    band_line: rgb(0x2c, 0x31, 0x3a),
    node_dim: rgb(0x27, 0x2b, 0x33),
    node_dim_ink: rgb(0x6b, 0x74, 0x80),
    pos: rgb(89, 200, 134),
    warn: rgb(240, 166, 70),
    ok_bg: rgb(0x14, 0x1b, 0x1a),
    warn_bg: rgb(0x22, 0x1c, 0x12),
    ok_dot: rgb(89, 200, 134),
    danger: rgb(0xef, 0x44, 0x44),
};

// ---- fonts ----

fn fam(n: &str) -> FontFamily {
    FontFamily::Name(n.into())
}
/// IBM Plex Sans, regular / medium / semibold.
pub fn sans(s: f32) -> FontId {
    FontId::new(s, FontFamily::Proportional)
}
pub fn med(s: f32) -> FontId {
    FontId::new(s, fam("med"))
}
pub fn semi(s: f32) -> FontId {
    FontId::new(s, fam("semi"))
}
/// IBM Plex Mono, regular / medium / semibold.
pub fn mono(s: f32) -> FontId {
    FontId::new(s, FontFamily::Monospace)
}
pub fn monom(s: f32) -> FontId {
    FontId::new(s, fam("monom"))
}
pub fn monos(s: f32) -> FontId {
    FontId::new(s, fam("monos"))
}

pub fn fonts() -> FontDefinitions {
    let mut f = FontDefinitions::default();
    // egui's own fonts stay on as fallbacks, for the symbols Plex doesn't have (✕ ↶ ☀ ☾ …).
    let mut fallbacks: Vec<String> = f.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    // Hack has the arrows and shapes (← → ▶ ■ ●) the others lack
    fallbacks.insert(0, "Hack".into());
    for (k, b) in [
        ("plex-sans", &include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf")[..]),
        ("plex-sans-med", &include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf")[..]),
        ("plex-sans-semi", &include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf")[..]),
        ("plex-mono", &include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")[..]),
        ("plex-mono-med", &include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf")[..]),
        ("plex-mono-semi", &include_bytes!("../assets/fonts/IBMPlexMono-SemiBold.ttf")[..]),
    ] {
        f.font_data.insert(k.into(), Arc::new(FontData::from_static(b)));
    }
    let with_fb = |first: &str| {
        let mut v = vec![first.to_string()];
        v.extend(fallbacks.iter().cloned());
        v
    };
    f.families.insert(FontFamily::Proportional, with_fb("plex-sans"));
    f.families.insert(FontFamily::Monospace, with_fb("plex-mono"));
    f.families.insert(fam("med"), with_fb("plex-sans-med"));
    f.families.insert(fam("semi"), with_fb("plex-sans-semi"));
    f.families.insert(fam("monom"), with_fb("plex-mono-med"));
    f.families.insert(fam("monos"), with_fb("plex-mono-semi"));
    f
}

/// egui's own widgets (text fields, combo boxes, scroll bars) styled to match.
pub fn apply(ctx: &egui::Context, p: &Pal) {
    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.shell;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.line);
    v.window_corner_radius = CornerRadius::same(14);
    v.window_shadow = Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(if p.dark { 90 } else { 20 }) };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(if p.dark { 90 } else { 24 }) };
    v.extreme_bg_color = p.card;
    v.faint_bg_color = p.row;
    v.code_bg_color = p.chip;
    v.selection.bg_fill = p.accent.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.hyperlink_color = p.accent;
    v.text_cursor.stroke = Stroke::new(1.5, p.accent);
    v.override_text_color = Some(p.ink);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(8);
        w.bg_fill = p.card;
        w.weak_bg_fill = p.card;
        w.bg_stroke = Stroke::new(1.0, p.line);
        w.fg_stroke = Stroke::new(1.0, p.ink2);
        w.expansion = 0.0;
    }
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    v.widgets.open.bg_stroke = Stroke::new(1.0, p.accent);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.ink);
    v.widgets.noninteractive.bg_fill = p.card;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(9.0, 6.0);
        s.spacing.interact_size.y = 26.0;
        s.spacing.scroll.bar_width = 6.0;
        s.spacing.scroll.floating = true;
        s.interaction.selectable_labels = false;
        s.text_styles.insert(egui::TextStyle::Body, sans(14.0));
        s.text_styles.insert(egui::TextStyle::Button, med(12.0));
        s.text_styles.insert(egui::TextStyle::Small, sans(11.0));
        s.text_styles.insert(egui::TextStyle::Monospace, mono(12.0));
    });
}

// ---- widgets ----

pub fn text(ui: &mut Ui, t: impl Into<String>, font: FontId, c: Color32) -> Response {
    ui.add(egui::Label::new(RichText::new(t).font(font).color(c)).selectable(false))
}

/// Wrapped text in the available width.
pub fn para(ui: &mut Ui, t: impl Into<String>, font: FontId, c: Color32) -> Response {
    ui.add(egui::Label::new(RichText::new(t).font(font).color(c)).wrap().selectable(false))
}

/// The small upper-case mono labels ("FIG. 1", "DEVICE").
pub fn caps(ui: &mut Ui, p: &Pal, t: &str) -> Response {
    ui.add(egui::Label::new(RichText::new(t.to_uppercase()).font(monos(10.0)).color(p.muted).extra_letter_spacing(1.4)).selectable(false))
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Normal,
    /// Filled with the accent: the primary action, or a toggle that is on.
    On,
    Small,
    SmallOn,
}

/// The app's button: white with a line border, accent on hover, accent-filled when "on".
pub fn button(ui: &mut Ui, p: &Pal, label: &str, kind: Kind, enabled: bool) -> Response {
    let (on, small) = match kind {
        Kind::Normal => (false, false),
        Kind::On => (true, false),
        Kind::Small => (false, true),
        Kind::SmallOn => (true, true),
    };
    let font = med(if small { 11.5 } else { 12.0 });
    let pad = if small { Vec2::new(11.0, 6.0) } else { Vec2::new(12.0, 9.0) };
    let (undo_icon, label) = match label.strip_prefix("↶ ") {
        Some(l) => (true, l),
        None => (false, label),
    };
    let cross_icon = label == "✕";
    let label = if cross_icon { "  " } else { label };
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, Color32::WHITE);
    let icon_w = if undo_icon { 14.0 } else { 0.0 };
    let size = galley.size() + pad * 2.0 + Vec2::new(icon_w, 0.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let hov = enabled && resp.hovered();
    let (bg, fg, st) = if on {
        (p.accent, p.accent_ink, p.accent)
    } else {
        (p.btn, if hov { p.accent } else { p.ink2 }, if hov { p.accent } else { p.line })
    };
    let a = if enabled { 1.0 } else { 0.4 };
    let r = if small { 7 } else { 8 };
    ui.painter().rect(rect, r, bg.gamma_multiply(a), Stroke::new(1.0, st.gamma_multiply(a)), egui::StrokeKind::Inside);
    let tpos = rect.center() - galley.size() / 2.0 + Vec2::new(icon_w / 2.0, 0.0);
    if undo_icon {
        let c = egui::pos2(tpos.x - 9.0, rect.center().y + 0.5);
        let st = Stroke::new(1.3, fg.gamma_multiply(a));
        let pts: Vec<egui::Pos2> = (0..=10).map(|k| {
            let t = std::f32::consts::PI * (1.15 - 1.25 * k as f32 / 10.0);
            egui::pos2(c.x + 3.8 * t.cos(), c.y - 3.8 * t.sin())
        }).collect();
        let tip = pts[0];
        ui.painter().add(egui::Shape::line(pts, st));
        ui.painter().line_segment([tip, tip + Vec2::new(-0.5, -3.2)], st);
        ui.painter().line_segment([tip, tip + Vec2::new(3.0, -1.2)], st);
    }
    if cross_icon {
        cross(ui.painter(), rect.center(), 3.5, Stroke::new(1.3, fg.gamma_multiply(a)));
    }
    ui.painter().galley_with_override_text_color(tpos, galley, fg.gamma_multiply(a));
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// A button with a fixed size and its label centred.
pub fn button_sized(ui: &mut Ui, p: &Pal, label: &str, size: Vec2, on: bool, enabled: bool, font: FontId) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let hov = enabled && resp.hovered();
    let (bg, fg, st) = if on { (p.accent, p.accent_ink, p.accent) } else { (p.btn, if hov { p.accent } else { p.ink2 }, if hov { p.accent } else { p.line }) };
    let a = if enabled { 1.0 } else { 0.45 };
    ui.painter().rect(rect, 8, bg.gamma_multiply(a), Stroke::new(1.0, st.gamma_multiply(a)), egui::StrokeKind::Inside);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, font, fg.gamma_multiply(a));
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// The big accent button (Write, Detect).
pub fn primary(ui: &mut Ui, p: &Pal, label: &str, width: f32, height: f32, font: FontId, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), if enabled { Sense::click() } else { Sense::hover() });
    let a = if enabled { 1.0 } else { 0.6 };
    let sh = Shadow { offset: [0, 6], blur: 18, spread: 0, color: if p.dark { Color32::from_black_alpha(110) } else { p.accent.gamma_multiply(0.25) } };
    ui.painter().add(sh.as_shape(rect, CornerRadius::same(10)));
    let fill = if enabled && resp.hovered() { lighten(p.accent, 0.05) } else { p.accent };
    ui.painter().rect_filled(rect, 10, fill.gamma_multiply(a));
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, font, p.accent_ink);
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

pub fn lighten(c: Color32, k: f32) -> Color32 {
    let f = |v: u8| (v as f32 + (255.0 - v as f32) * k) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

/// A link-style text button ("connect", "FR files…").
pub fn link(ui: &mut Ui, p: &Pal, t: &str, font: FontId, color: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(t.to_string(), font, color);
    let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::click());
    let c = if resp.hovered() { p.accent } else { color };
    ui.painter().galley_with_override_text_color(rect.min, galley, c);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A segmented toggle (EQ / FR, rigs, theme). Returns the index clicked.
pub fn segmented(ui: &mut Ui, p: &Pal, items: &[&str], sel: usize, font: FontId, pad: Vec2, filled: bool) -> Option<usize> {
    let mut clicked = None;
    egui::Frame::new().fill(p.chip).corner_radius(8).inner_margin(3).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.horizontal(|ui| {
            let rtl = ui.layout().prefer_right_to_left();
            let order: Vec<usize> = if rtl { (0..items.len()).rev().collect() } else { (0..items.len()).collect() };
            for i in order {
                let t = &items[i];
                let on = i == sel;
                let galley = ui.painter().layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE);
                let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::click());
                let (bg, fg) = if on {
                    if filled {
                        (p.accent, p.accent_ink)
                    } else {
                        (p.card, p.accent)
                    }
                } else {
                    (Color32::TRANSPARENT, if resp.hovered() { p.ink2 } else { p.muted })
                };
                ui.painter().rect_filled(rect, 6, bg);
                ui.painter().galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, fg);
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    clicked = Some(i);
                }
            }
        });
    });
    clicked
}

/// The section heading in the right-hand rail: a rule, a number and a title, a link on the right.
pub fn step(ui: &mut Ui, p: &Pal, num: &str, title: &str, link_text: Option<&str>) -> bool {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, 2.0), Sense::hover());
    ui.painter().rect_filled(r, 0, p.ink2);
    let mut clicked = false;
    ui.horizontal(|ui| {
        if !num.is_empty() {
            text(ui, num, monos(12.0), p.accent);
        }
        text(ui, title, semi(14.0), p.ink);
        if let Some(l) = link_text {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                clicked = link(ui, p, l, mono(11.0), p.muted).clicked();
            });
        }
    });
    clicked
}

/// A note box: ok (green) or warn (amber), with a dot.
pub fn note(ui: &mut Ui, p: &Pal, t: &str, warn: bool, dot: bool) {
    let (bg, fg) = if warn { (p.warn_bg, p.warn) } else { (p.ok_bg, p.pos) };
    egui::Frame::new().fill(bg).corner_radius(9).inner_margin(Margin::symmetric(11, 9)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            if dot {
                let (r, _) = ui.allocate_exact_size(Vec2::new(7.0, 15.0), Sense::hover());
                ui.painter().circle_filled(egui::pos2(r.center().x, r.top() + 8.0), 3.5, fg);
            }
            para(ui, t, mono(11.5), fg);
        });
    });
}

/// A text field with the app's look.
pub fn field(ui: &mut Ui, p: &Pal, s: &mut String, width: f32, hint: &str, font: FontId) -> Response {
    let _ = p;
    ui.add(egui::TextEdit::singleline(s).desired_width(width).hint_text(hint).font(font).margin(Margin::symmetric(9, 7)))
}

/// A list row (saved EQs, curves): name, optional tag and meta, and a delete cross on hover.
/// Returns (row clicked, delete clicked).
pub fn list_row(ui: &mut Ui, p: &Pal, name: &str, tag: Option<&str>, meta: &str, current: bool, deletable: bool) -> (bool, bool) {
    let w = ui.available_width();
    let h = 34.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let hov = resp.hovered();
    let bg = if hov { p.chip } else { p.row };
    ui.painter().rect_filled(rect, 8, bg);
    if current {
        ui.painter().rect_stroke(rect.shrink(0.75), 8, Stroke::new(1.5, p.accent), egui::StrokeKind::Inside);
    }
    let mut x = rect.left() + 10.0;
    let cy = rect.center().y;
    let g = ui.painter().layout_no_wrap(name.to_string(), sans(12.5), p.ink);
    let nw = g.size().x;
    ui.painter().galley(egui::pos2(x, cy - g.size().y / 2.0), g, p.ink);
    x += nw + 9.0;
    if let Some(t) = tag {
        let g = ui.painter().layout_no_wrap(t.to_string(), mono(9.0), p.accent);
        let tr = egui::Rect::from_min_size(egui::pos2(x, cy - 8.0), Vec2::new(g.size().x + 12.0, 16.0));
        ui.painter().rect_stroke(tr, 99, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
        ui.painter().galley(tr.center() - g.size() / 2.0, g, p.accent);
    }
    let mut del = false;
    let mut right = rect.right() - 10.0;
    if deletable {
        let xr = egui::Rect::from_center_size(egui::pos2(right - 6.0, cy), Vec2::splat(16.0));
        let xresp = ui.interact(xr, resp.id.with("del"), Sense::click());
        if hov || xresp.hovered() {
            let c = if xresp.hovered() { p.danger } else { p.faint };
            cross(ui.painter(), xr.center(), 4.0, Stroke::new(1.4, c));
        }
        if xresp.clicked() {
            del = true;
        }
        right -= 22.0;
    }
    if !meta.is_empty() {
        ui.painter().text(egui::pos2(right, cy), egui::Align2::RIGHT_CENTER, meta, mono(10.5), p.faint);
    }
    (resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !del, del)
}

pub fn cross(painter: &egui::Painter, c: egui::Pos2, r: f32, st: Stroke) {
    painter.line_segment([c + Vec2::new(-r, -r), c + Vec2::new(r, r)], st);
    painter.line_segment([c + Vec2::new(-r, r), c + Vec2::new(r, -r)], st);
}

/// The small square window buttons in the header (minimise, close, disconnect).
pub fn win_button(ui: &mut Ui, p: &Pal, icon: WinIcon, danger_hover: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
    let hov = resp.hovered();
    let (bg, st, fg) = if hov && danger_hover {
        (p.danger, p.danger, Color32::WHITE)
    } else if hov {
        (Color32::TRANSPARENT, p.accent, p.accent)
    } else {
        (Color32::TRANSPARENT, p.line, p.muted)
    };
    ui.painter().rect(rect, 6, bg, Stroke::new(1.0, st), egui::StrokeKind::Inside);
    let c = rect.center();
    let s = Stroke::new(1.4, fg);
    match icon {
        WinIcon::Close => cross(ui.painter(), c, 4.0, s),
        WinIcon::Min => {
            ui.painter().line_segment([c + Vec2::new(-4.5, 0.0), c + Vec2::new(4.5, 0.0)], s);
        }
        WinIcon::Eject => {
            let pts = vec![c + Vec2::new(-4.5, 1.0), c + Vec2::new(4.5, 1.0), c + Vec2::new(0.0, -4.5)];
            ui.painter().add(egui::Shape::convex_polygon(pts, fg, Stroke::NONE));
            ui.painter().line_segment([c + Vec2::new(-4.5, 4.0), c + Vec2::new(4.5, 4.0)], s);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[derive(Clone, Copy)]
pub enum WinIcon {
    Close,
    Min,
    Eject,
}

/// The app's mark: the gradient tile with the response curve, drawn rather than loaded.
pub fn mark(ui: &mut Ui, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_mark(ui.painter(), rect);
    resp
}

pub fn paint_mark(painter: &egui::Painter, rect: egui::Rect) {
    let s = rect.width() / 1024.0;
    let at = |x: f32, y: f32| egui::pos2(rect.left() + x * s, rect.top() + y * s);
    // gradient tile with rounded corners: a fan of triangles over a rounded outline, coloured
    // along the diagonal
    let (c0, c1) = (Color32::from_rgb(0x5b, 0x6e, 0xf5), Color32::from_rgb(0x34, 0x37, 0x9f));
    let r = rect.width() * 0.28;
    let mut outline = Vec::new();
    for (cx, cy, a0) in [(rect.right() - r, rect.top() + r, -90.0f32), (rect.right() - r, rect.bottom() - r, 0.0), (rect.left() + r, rect.bottom() - r, 90.0), (rect.left() + r, rect.top() + r, 180.0)] {
        for k in 0..=8 {
            let a = (a0 + k as f32 * 90.0 / 8.0).to_radians();
            outline.push(egui::pos2(cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    let col = |q: egui::Pos2| {
        let t = (((q.x - rect.left()) + (q.y - rect.top())) / (rect.width() * 2.0)).clamp(0.0, 1.0);
        lerp_color(c0, c1, t)
    };
    let mut mesh = egui::Mesh::default();
    let center = rect.center();
    mesh.colored_vertex(center, col(center));
    for q in &outline {
        mesh.colored_vertex(*q, col(*q));
    }
    let n = outline.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(egui::Shape::mesh(mesh));
    let st = Stroke::new(74.0 * s, Color32::WHITE);
    let seg = |a: (f32, f32), b: (f32, f32), c: (f32, f32), d: (f32, f32)| {
        egui::epaint::CubicBezierShape::from_points_stroke([at(a.0, a.1), at(b.0, b.1), at(c.0, c.1), at(d.0, d.1)], false, Color32::TRANSPARENT, st)
    };
    painter.add(seg((150.0, 588.0), (280.0, 588.0), (300.0, 372.0), (448.0, 372.0)));
    painter.add(seg((448.0, 372.0), (566.0, 372.0), (566.0, 664.0), (700.0, 664.0)));
    painter.add(seg((700.0, 664.0), (806.0, 664.0), (828.0, 536.0), (874.0, 536.0)));
    for (x, y) in [(150.0, 588.0), (874.0, 536.0)] {
        painter.circle_filled(at(x, y), 37.0 * s, Color32::WHITE);
    }
    painter.circle_filled(at(448.0, 372.0), 62.0 * s, Color32::WHITE);
    painter.circle_filled(at(700.0, 664.0), 62.0 * s, Color32::WHITE);
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}
