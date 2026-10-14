// The main window: the header (which is also the title bar), the connect screen, and the
// workspace (Fig. 1 response graph and Table 1 bands on the left, the numbered workflow on the
// right). Dialogs are in dialogs.rs.

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, CursorIcon, Id, Key, Layout, Margin, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};

use crate::app::{bank_short, is_off_slot, App, GraphMode, Modal};
use crate::eq::{self, fixed, Field};
use crate::fit::FitMode;
use crate::fr;
use crate::theme::{self, button, caps, link, mono, monom, monos, med, para, sans, semi, step, text, Kind, Pal, WinIcon};

const DB_MAX: f64 = 12.0;

pub fn draw(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    egui::Panel::top("header")
        .exact_size(64.0)
        .frame(egui::Frame::new().fill(p.shell).inner_margin(Margin { left: if cfg!(target_os = "macos") { 84 } else { 24 }, right: 24, top: 0, bottom: 0 }))
        .show(ui, |ui| header(app, ui));
    if app.connected {
        egui::Panel::right("rail")
            .exact_size(300.0 + 24.0 + 14.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.shell).inner_margin(Margin { left: 14, right: 24, top: 14, bottom: 16 }))
            .show(ui, |ui| rail(app, ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.shell).inner_margin(Margin { left: 24, right: 14, top: 14, bottom: 16 }))
            .show(ui, |ui| workspace(app, ui));
    } else {
        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.shell).inner_margin(24)).show(ui, |ui| connect(app, ui));
    }
    let ctx = ui.ctx().clone();
    overlays(app, &ctx);
    if !cfg!(target_os = "macos") {
        resize_edges(&ctx);
    }
}

// ---- header ----

fn header(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let full = ui.max_rect();
    // the frameless window's drag region: anything in the header that isn't a control
    let bg = ui.interact(full, Id::new("titlebar"), Sense::click_and_drag());
    if bg.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if bg.double_clicked() {
        let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
    }
    ui.painter().rect_filled(Rect::from_min_max(pos2(full.left(), full.bottom() - 2.0), full.right_bottom()), 0, p.ink2);
    let width = ui.ctx().content_rect().width();
    ui.horizontal(|ui| {
        ui.set_height(62.0);
        ui.spacing_mut().item_spacing.x = 0.0;
        // brand
        ui.add_space(0.0);
        theme::mark(ui, 28.0);
        ui.add_space(12.0);
        ui.vertical(|ui| {
            ui.add_space(if width > 1400.0 { 13.0 } else { 20.0 });
            text(ui, "FreeDSP Studio", semi(18.0), p.ink);
            if width > 1400.0 {
                ui.add_space(-2.0);
                caps(ui, &p, "Parametric EQ for USB-C DSP cables");
            }
        });
        ui.add_space(22.0);
        vline(ui, &p);
        if app.connected {
            spec_cells(app, ui, width);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            if !cfg!(target_os = "macos") {
                if theme::win_button(ui, &p, WinIcon::Close, true).on_hover_text("Close").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ui.add_space(-6.0);
                if theme::win_button(ui, &p, WinIcon::Min, false).on_hover_text("Minimise").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
            }
            theme_switch(app, ui);
            if app.cable.is_some() && theme::win_button(ui, &p, WinIcon::Eject, false).on_hover_text("Disconnect").clicked() {
                app.disconnect();
            }
            if app.connected {
                top_links(app, ui);
            }
        });
    });
}

fn vline(ui: &mut Ui, p: &Pal) {
    let (r, _) = ui.allocate_exact_size(vec2(1.0, 62.0), Sense::hover());
    ui.painter().rect_filled(r, 0, p.line);
}

fn spec_cells(app: &mut App, ui: &mut Ui, width: f32) {
    let p = app.pal;
    let (name, id, bands) = if let Some(c) = &app.cable {
        let b = if -c.gain_min == c.gain_max { format!("{} · ±{} dB", c.bands, eq::js_num(c.gain_max)) } else { format!("{} · −{} / +{} dB", c.bands, eq::js_num(-c.gain_min), eq::js_num(c.gain_max)) };
        (c.name.clone(), Some(format!("{:04X}:{:04X}", c.vid, c.pid)), b)
    } else if let Some(o) = &app.offline {
        let t = &o.target;
        let b = if -t.g_min == t.g_max { format!("{} · ±{} dB", o.bands, eq::js_num(t.g_max)) } else { format!("{} · −{} / +{} dB", o.bands, eq::js_num(-t.g_min), eq::js_num(t.g_max)) };
        (t.name.clone(), None, b)
    } else {
        return;
    };
    let status = if let Some(c) = &app.cable {
        format!("{}{}", c.firmware.as_ref().map(|f| format!("connected · fw {}", f)).unwrap_or_else(|| "connected".into()), if c.tested { "" } else { " · untested model" })
    } else {
        "offline · no device".into()
    };
    let warn = app.write_note.is_some();
    let cell = |ui: &mut Ui, label: &str, f: &mut dyn FnMut(&mut Ui)| {
        ui.add_space(18.0);
        ui.vertical(|ui| {
            ui.add_space(12.0);
            caps(ui, &p, label);
            ui.add_space(2.0);
            f(ui);
        });
        ui.add_space(18.0);
        vline(ui, &p);
    };
    cell(ui, "Device", &mut |ui| {
        text(ui, &name, semi(13.5), p.ink);
    });
    if let (Some(id), true) = (&id, width > 1300.0) {
        cell(ui, "USB ID", &mut |ui| {
            text(ui, id, mono(12.5), p.ink2);
        });
    }
    if width > 1120.0 {
        cell(ui, "Bands", &mut |ui| {
            text(ui, &bands, mono(12.5), p.ink2);
        });
    }
    cell(ui, "Status", &mut |ui| {
        ui.horizontal(|ui| {
            let c = if warn { p.warn } else { p.ok_dot };
            let (r, _) = ui.allocate_exact_size(vec2(13.0, 16.0), Sense::hover());
            ui.painter().circle_filled(pos2(r.left() + 3.0, r.center().y), 3.0, c);
            text(ui, &status, mono(12.5), if warn { p.warn } else { p.ink2 });
        });
    });
}

fn top_links(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    ui.spacing_mut().item_spacing.x = 16.0;
    let f = mono(11.0);
    if link(ui, &p, "about", f.clone(), p.ink2).on_hover_text("About FreeDSP Studio").clicked() {
        app.open_modal(Modal::About);
    }
    if link(ui, &p, "log", f.clone(), p.ink2).on_hover_text("Diagnostics log").clicked() {
        app.open_modal(Modal::Log);
    }
    if link(ui, &p, "backup", f.clone(), p.ink2).on_hover_text("Back up or restore your EQs and FR curves").clicked() {
        app.open_modal(Modal::Backup);
    }
    if link(ui, &p, "saved", f.clone(), p.ink2).on_hover_text("Saved EQs and built-in starting points").clicked() {
        app.open_modal(Modal::Saved);
    }
    if app.offline.is_some() && link(ui, &p, "connect", f.clone(), p.ink2).on_hover_text("Leave offline mode and look for a device").clicked() {
        app.disconnect();
        app.detect();
    }
    if app.cable.is_some() {
        let flashing = app.read_flash.map(|t| t > std::time::Instant::now()).unwrap_or(false);
        if link(ui, &p, if flashing { "read ✔" } else { "read cable" }, f, p.ink2).on_hover_text("Read the cable's current custom bank").clicked() {
            app.read_from_cable();
            app.read_flash = Some(std::time::Instant::now() + std::time::Duration::from_millis(1100));
            app.read_note = app.caps.gen == 1;
        }
    }
}

fn theme_switch(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let ctx = ui.ctx().clone();
    egui::Frame::new().fill(p.chip).corner_radius(8).inner_margin(3).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.horizontal(|ui| {
            let order = if ui.layout().prefer_right_to_left() { [true, false] } else { [false, true] };
            for dark in order {
                let (r, resp) = ui.allocate_exact_size(vec2(30.0, 24.0), Sense::click());
                let on = p.dark == dark;
                if on {
                    ui.painter().rect_filled(r, 6, p.card);
                }
                let c = if on { p.accent } else if resp.hovered() { p.ink2 } else { p.muted };
                let m = r.center();
                if dark {
                    // crescent
                    ui.painter().circle_filled(m, 5.5, c);
                    ui.painter().circle_filled(m + vec2(2.8, -2.2), 4.8, if on { p.card } else { p.chip });
                } else {
                    ui.painter().circle_stroke(m, 2.8, Stroke::new(1.3, c));
                    for k in 0..8 {
                        let a = k as f32 * std::f32::consts::PI / 4.0;
                        let d = vec2(a.cos(), a.sin());
                        ui.painter().line_segment([m + d * 4.6, m + d * 6.3], Stroke::new(1.2, c));
                    }
                }
                if resp.on_hover_text(if dark { "Dark" } else { "Light" }).on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    app.set_theme(&ctx, dark);
                }
            }
        });
    });
}

// ---- connect screen ----

fn connect(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let avail = ui.available_rect_before_wrap();
    let w = 460.0f32.min(avail.width());
    let h_est = 300.0 + app.device_list.len() as f32 * 52.0;
    let top = avail.center().y - h_est / 2.0;
    let rect = Rect::from_min_size(pos2(avail.center().x - w / 2.0, top.max(avail.top())), vec2(w, h_est));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Center)));
    card_frame(&p, true).inner_margin(Margin::symmetric(32, 28)).show(&mut child, |ui| {
        ui.set_width(w - 64.0);
        ui.vertical_centered(|ui| {
            text(ui, &app.connect_title, semi(18.0), p.ink);
            ui.add_space(2.0);
            para(ui, "Plug in your DSP cable, then click Detect. Nothing is changed until you Write.", sans(13.0), p.muted);
            ui.add_space(14.0);
            let list = app.device_list.clone();
            for d in &list {
                let fam = match d.gen {
                    3 => "FiiO".to_string(),
                    4 => "KT Micro".into(),
                    5 => "Fosi Audio".into(),
                    g => format!("gen {}", g),
                };
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
                let st = if resp.hovered() { p.accent } else { p.line };
                ui.painter().rect(r, 10, p.card, Stroke::new(1.0, st), egui::StrokeKind::Inside);
                ui.painter().text(pos2(r.left() + 14.0, r.center().y), Align2::LEFT_CENTER, &d.name, sans(14.0), p.ink);
                ui.painter().text(pos2(r.right() - 14.0, r.center().y), Align2::RIGHT_CENTER, format!("{:x}:{:x} · {}", d.vid, d.pid, fam), mono(11.0), p.muted);
                if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    app.open_cable(Some((d.vid as u16, d.pid as u16)));
                }
                ui.add_space(8.0);
            }
            if !list.is_empty() {
                ui.add_space(10.0);
            }
            let busy = app.worker.busy();
            if theme::primary(ui, &p, if busy { "Looking…" } else { "Detect cable" }, 160.0, 44.0, semi(15.0), !busy).clicked() {
                app.detect();
            }
            ui.add_space(22.0);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(r, 0, p.line);
            ui.add_space(16.0);
            text(ui, "Or work without a device", sans(12.0), p.muted);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let tw = 260.0 + 70.0 + 60.0 + 16.0;
                ui.add_space(((ui.available_width() - tw) / 2.0).max(0.0));
                let cur = app.targets.get(&app.off_target).map(|t| offline_label(t)).unwrap_or_default();
                egui::ComboBox::from_id_salt("offTarget").width(250.0).selected_text(egui::RichText::new(cur).font(sans(12.0))).show_ui(ui, |ui| {
                    let ids: Vec<String> = app.targets.keys().cloned().collect();
                    for id in ids {
                        let t = app.targets[&id].clone();
                        ui.selectable_value(&mut app.off_target, id, egui::RichText::new(offline_label(&t)).font(sans(12.0)));
                    }
                });
                text(ui, "bands", sans(12.0), p.muted).on_hover_text("How many bands to design with");
                let mut v = app.off_bands as f64;
                if num_field(ui, &p, Id::new("offBands"), &mut v, 52.0, 0) {
                    app.off_bands = (eq::jround(v) as usize).clamp(1, 31);
                }
                if button(ui, &p, "Start", Kind::Normal, true).clicked() {
                    app.enter_offline();
                }
            });
        });
    });
}

fn offline_label(t: &eq::Target) -> String {
    if t.id == "generic" {
        "Generic".into()
    } else {
        format!("{} · bank set{}", t.name, if t.tested { "" } else { " (untested)" })
    }
}

pub fn card_frame(p: &Pal, raised: bool) -> egui::Frame {
    egui::Frame::new().fill(p.card).stroke(Stroke::new(1.0, p.line)).corner_radius(14).shadow(if raised {
        egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(if p.dark { 90 } else { 14 }) }
    } else {
        egui::Shadow { offset: [0, 1], blur: 2, spread: 0, color: Color32::from_black_alpha(if p.dark { 76 } else { 10 }) }
    })
}

/// A number box that commits on Enter or when it loses focus. Returns true when committed.
pub fn num_field(ui: &mut Ui, p: &Pal, id: Id, v: &mut f64, width: f32, decimals: usize) -> bool {
    let _ = p;
    let mut s: String = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| fixed(*v, decimals));
    let resp = ui.add(
        egui::TextEdit::singleline(&mut s)
            .id(id.with("te"))
            .desired_width(width)
            .font(mono(12.0))
            .horizontal_align(Align::RIGHT)
            .margin(Margin::symmetric(9, 8)),
    );
    let mut done = false;
    if resp.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, s.clone()));
    }
    if resp.lost_focus() {
        let t = s.trim().replace(',', ".");
        if let Ok(x) = t.parse::<f64>() {
            *v = x;
            done = true;
        }
        ui.data_mut(|d| d.remove::<String>(id));
    }
    done
}

// ---- workspace ----

fn workspace(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    if app.read_note {
        egui::Frame::new().fill(p.chip).stroke(Stroke::new(1.0, p.line)).corner_radius(9).inner_margin(Margin::symmetric(12, 9)).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.set_width(ui.available_width());
                let w = ui.available_width() - 24.0;
                ui.allocate_ui(vec2(w, 0.0), |ui| {
                    para(ui, "Read-back gains are whole dB, the cable can only report integer values, so any decimals are rounded on read. There's nothing the app can do about that. For precise edits, save an EQ and tweak the saved copy rather than relying on the read-back.", mono(11.5), p.ink2);
                });
                if link(ui, &p, "✕", sans(12.0), p.muted).on_hover_text("Dismiss").clicked() {
                    app.read_note = false;
                }
            });
        });
        ui.add_space(8.0);
    }
    // Fig. 1 head
    ui.horizontal(|ui| {
        ui.set_height(32.0);
        caps(ui, &p, "Fig. 1").on_hover_text("");
        ui.add_space(6.0);
        let n = eq::active_count(&app.model);
        let title = if app.graph_mode == GraphMode::Fr && app.frs.meas_rec().is_some() {
            format!("Response vs target · {}", app.frs.cur_meas.clone().unwrap_or_default())
        } else {
            format!("EQ response, {} of {} bands", n, app.caps.bands)
        };
        text(ui, title, med(14.0), p.ink);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let has_prof = app.frs.active_profile().is_some();
            let mut items = vec!["EQ", "FR"];
            if has_prof && app.graph_mode == GraphMode::Fr {
                items.push("TREBLE");
            }
            let sel = if app.graph_mode == GraphMode::Eq { 0 } else { 1 };
            // TREBLE is a toggle beside the two modes
            if let Some(i) = gmode(ui, &p, &items, sel, app.treble_view) {
                match i {
                    0 => app.set_graph_mode(GraphMode::Eq),
                    1 => app.set_graph_mode(GraphMode::Fr),
                    _ => {
                        app.treble_view = !app.treble_view;
                        app.save_settings();
                    }
                }
            }
        });
    });
    ui.add_space(8.0);
    // Table 1 takes what it needs; the graph gets the rest
    let rows = app.model.len() as f32;
    let table_h = 12.0 + 32.0 + 8.0 + 4.0 + 26.0 + rows * 33.0 + 2.0;
    let graph_h = (ui.available_height() - table_h).max(150.0);
    let w = ui.available_width();
    let (grect, _) = ui.allocate_exact_size(vec2(w, graph_h), Sense::hover());
    graph(app, ui, grect);
    ui.add_space(12.0);
    // Table 1 head
    ui.horizontal(|ui| {
        ui.set_height(32.0);
        caps(ui, &p, "Table 1");
        ui.add_space(6.0);
        text(ui, "Bands", med(14.0), p.ink);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if button(ui, &p, "Export…", Kind::Small, true).on_hover_text("Save the EQ as a squig / Equalizer APO parametric .txt").clicked() {
                app.export_eq();
            }
            if button(ui, &p, "As text", Kind::Small, true).on_hover_text("Edit the EQ as text (squig / AutoEQ)").clicked() {
                app.open_modal(Modal::EqText);
            }
            let cu = app.can_undo();
            if button(ui, &p, "↶ Undo", Kind::Small, cu).on_hover_text("Undo last change").clicked() {
                app.undo();
            }
            if button(ui, &p, "Flatten", Kind::Small, true).on_hover_text("Reset all bands to flat").clicked() {
                app.flatten();
            }
            if button(ui, &p, "Sort", Kind::Small, true).on_hover_text("Order bands by frequency").clicked() {
                app.sort_bands();
            }
        });
    });
    ui.add_space(4.0);
    band_table(app, ui);
}

/// EQ / FR (and TREBLE when there is a profile): the first two are a choice, TREBLE a toggle.
fn gmode(ui: &mut Ui, p: &Pal, items: &[&str], sel: usize, treble_on: bool) -> Option<usize> {
    let mut clicked = None;
    egui::Frame::new().fill(p.chip).corner_radius(8).inner_margin(3).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.horizontal(|ui| {
            let rtl = ui.layout().prefer_right_to_left();
            let order: Vec<usize> = if rtl { (0..items.len()).rev().collect() } else { (0..items.len()).collect() };
            for i in order {
                let t = &items[i];
                if i == 2 && !rtl {
                    let (r, _) = ui.allocate_exact_size(vec2(1.0, 18.0), Sense::hover());
                    ui.painter().rect_filled(r, 0, p.line);
                }
                let on = if i == 2 { treble_on } else { i == sel };
                let g = ui.painter().layout_no_wrap(t.to_string(), monos(10.0), Color32::WHITE);
                let (r, resp) = ui.allocate_exact_size(g.size() + vec2(20.0, 8.0), Sense::click());
                if on {
                    ui.painter().rect_filled(r, 6, p.card);
                }
                let c = if on { p.accent } else { p.muted };
                ui.painter().galley_with_override_text_color(r.center() - g.size() / 2.0, g, c);
                let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                let resp = match i {
                    0 => resp.on_hover_text("EQ curve"),
                    1 => resp.on_hover_text("Frequency response vs target"),
                    _ => resp.on_hover_text("Show the measurement with your treble correction (smoothed rig level + your listening test above ~6 kHz), or the raw rig measurement"),
                };
                if resp.clicked() {
                    clicked = Some(i);
                }
                if i == 2 && rtl {
                    let (r, _) = ui.allocate_exact_size(vec2(1.0, 18.0), Sense::hover());
                    ui.painter().rect_filled(r, 0, p.line);
                }
            }
        });
    });
    clicked
}

// ---- Fig. 1 ----

fn f_to_x(f: f64) -> f64 {
    (f / 20.0).log10() / 1000f64.log10()
}

fn graph(app: &mut App, ui: &mut Ui, rect: Rect) {
    let p = app.pal;
    let painter = ui.painter_at(rect);
    painter.rect(rect, 10, p.card, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
    let inner = rect.shrink(1.0);
    let painter = painter.with_clip_rect(inner);
    if app.graph_mode == GraphMode::Fr && app.frs.meas_rec().is_some() {
        draw_fr(app, &painter, inner);
        return;
    }
    let (w, h) = (inner.width() as f64, inner.height() as f64);
    let db_min = app.db_min;
    let span = DB_MAX - db_min;
    let x = |f: f64| inner.left() + (f_to_x(f) * w) as f32;
    let y = |db: f64| inner.top() + ((1.0 - (db - db_min) / span) * h) as f32;
    // vertical gridlines + labels
    for f in [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0] {
        let px = x(f);
        painter.line_segment([pos2(px, inner.top()), pos2(px, inner.bottom())], Stroke::new(1.0, p.grid));
        let l = if f >= 1000.0 { format!("{}k", eq::js_num(f / 1000.0)) } else { eq::js_num(f) };
        painter.text(pos2(px + 6.0, inner.bottom() - 7.0), Align2::LEFT_BOTTOM, l, mono(10.0), p.faint);
    }
    // horizontal gridlines + dB labels
    let mut db = db_min + 3.0;
    while db < DB_MAX {
        let py = y(db);
        let zero = db == 0.0;
        painter.line_segment([pos2(inner.left(), py), pos2(inner.right(), py)], Stroke::new(if zero { 1.5 } else { 1.0 }, if zero { p.grid_zero } else { p.grid }));
        if (db as i64) % 6 == 0 {
            painter.text(pos2(inner.left() + 9.0, py - 3.0), Align2::LEFT_BOTTOM, format!("{}{}", if db > 0.0 { "+" } else { "" }, db as i64), mono(10.0), p.faint);
        }
        db += 3.0;
    }
    // individual band curves
    for b in &app.model {
        if b.gain == 0.0 {
            continue;
        }
        let pts: Vec<Pos2> = (0..=(w as usize / 2)).map(|i| {
            let px = (i * 2) as f64;
            pos2(inner.left() + px as f32, y(eq::mag_db(b, 20.0 * 1000f64.powf(px / w))))
        }).collect();
        painter.add(Shape::line(pts, Stroke::new(1.0, p.band_line)));
    }
    // combined curve + area fill
    let n = w as usize;
    let pts: Vec<Pos2> = (0..=n).map(|i| {
        let px = i as f64;
        pos2(inner.left() + px as f32, y(eq::combined_at(&app.model, 20.0 * 1000f64.powf(px / w)) + app.preamp))
    }).collect();
    let mut mesh = egui::Mesh::default();
    let top_c = p.accent_fill;
    for (i, q) in pts.iter().enumerate() {
        let t = ((q.y - inner.top()) / inner.height()).clamp(0.0, 1.0);
        mesh.colored_vertex(*q, top_c.gamma_multiply(1.0 - t));
        mesh.colored_vertex(pos2(q.x, inner.bottom()), Color32::TRANSPARENT);
        if i > 0 {
            let k = (i * 2) as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k, k + 1);
        }
    }
    painter.add(Shape::mesh(mesh));
    painter.add(Shape::line(pts, Stroke::new(2.5, p.accent)));
    nodes(app, ui, inner);
}

fn nodes(app: &mut App, ui: &mut Ui, inner: Rect) {
    let p = app.pal;
    let (w, h) = (inner.width() as f64, inner.height() as f64);
    let db_min = app.db_min;
    let span = DB_MAX - db_min;
    let g_max = app.cur_g_max();
    let mut changed = false;
    for i in 0..app.model.len() {
        let b = app.model[i];
        let cx = inner.left() + (eq::clamp(f_to_x(b.freq), 0.0, 1.0) * w) as f32;
        let cy = inner.top() + (eq::clamp((1.0 - (b.gain - db_min) / span) * 100.0, 2.0, 98.0) / 100.0 * h) as f32;
        let c = pos2(cx, cy);
        let id = Id::new(("node", i));
        let r = Rect::from_center_size(c, Vec2::splat(22.0));
        let resp = ui.interact(r, id, Sense::drag());
        if resp.drag_started() {
            app.push_undo();
            app.drag_node = Some(i);
            app.edit.clear();
        }
        if resp.dragged() || resp.drag_started() {
            if let Some(ptr) = resp.interact_pointer_pos() {
                let px = eq::clamp(((ptr.x - inner.left()) / inner.width()) as f64, 0.0, 1.0);
                let py = eq::clamp(((ptr.y - inner.top()) / inner.height()) as f64, 0.0, 1.0);
                app.model[i].freq = eq::clamp(eq::jround(20.0 * 1000f64.powf(px)), eq::F_MIN, eq::F_MAX);
                app.model[i].gain = eq::clamp(eq::round1((1.0 - py) * span + db_min), app.caps.g_min, g_max);
                changed = true;
            }
        }
        if resp.drag_stopped() {
            app.drag_node = None;
        }
        let active = b.gain != 0.0;
        let painter = ui.painter_at(inner);
        let sh = egui::Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(if p.dark { 128 } else { 51 }) };
        painter.add(sh.as_shape(Rect::from_center_size(c, Vec2::splat(22.0)), egui::CornerRadius::same(11)));
        painter.circle(c, 11.0, if active { p.accent } else { p.node_dim }, Stroke::new(2.0, p.card));
        painter.text(c, Align2::CENTER_CENTER, format!("{}", i + 1), monos(10.0), if active { p.accent_ink } else { p.node_dim_ink });
        let cur = if app.drag_node == Some(i) { CursorIcon::Grabbing } else { CursorIcon::Grab };
        resp.on_hover_cursor(cur);
    }
    if changed {
        ui.ctx().request_repaint();
    }
}

fn draw_fr(app: &mut App, painter: &egui::Painter, inner: Rect) {
    let p = app.pal;
    let Some(meas) = app.cur_meas_sm() else { return };
    let tgt = app.cur_tgt_sm();
    let prof = if app.treble_view { app.active_profile() } else { None };
    let (w, h) = (inner.width() as f64, inner.height() as f64);
    let measured = |f: f64| -> f64 {
        match &prof {
            Some(pr) => fr::ear_meas_at(&meas, pr, f),
            None => fr::interp(&meas.raw, f),
        }
    };
    let target = |f: f64| -> Option<f64> {
        tgt.as_ref().map(|t| if prof.is_some() { fr::fit_tgt_at(t, f) } else { fr::interp(&t.raw, f) })
    };
    let eq1k = eq::combined_at(&app.model, 1000.0);
    let model = app.model.clone();
    let result = |f: f64| measured(f) + eq::combined_at(&model, f) - eq1k; // EQ'd, re-normalised at 1 kHz
    // auto y-range over the visible curves
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut px = 0.0;
    while px <= w {
        let f = 20.0 * 1000f64.powf(px / w);
        let mut vs = vec![fr::interp(&meas.raw, f), result(f)];
        if let Some(t) = target(f) {
            vs.push(t);
        }
        for v in vs {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        px += 4.0;
    }
    lo = ((lo - 4.0) / 5.0).floor() * 5.0;
    hi = ((hi + 4.0) / 5.0).ceil() * 5.0;
    if hi - lo < 20.0 {
        hi = lo + 20.0;
    }
    let x = |f: f64| inner.left() + (f_to_x(f) * w) as f32;
    let y = |db: f64| inner.top() + (h - (db - lo) / (hi - lo) * h) as f32;
    for f in [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0] {
        let px = x(f);
        painter.line_segment([pos2(px, inner.top()), pos2(px, inner.bottom())], Stroke::new(1.0, p.grid));
        let l = if f >= 1000.0 { format!("{}k", eq::js_num(f / 1000.0)) } else { eq::js_num(f) };
        painter.text(pos2(px + 6.0, inner.bottom() - 7.0), Align2::LEFT_BOTTOM, l, mono(10.0), p.faint);
    }
    let mut db = lo;
    while db <= hi {
        let py = y(db);
        painter.line_segment([pos2(inner.left(), py), pos2(inner.right(), py)], Stroke::new(1.0, p.grid));
        painter.text(pos2(inner.left() + 9.0, py - 3.0), Align2::LEFT_BOTTOM, eq::js_num(db), mono(10.0), p.faint);
        db += 5.0;
    }
    // ear-corrected view: shade where the listening test replaces the rig's detail
    if prof.is_some() {
        let (x0, x1, xf) = (x(fr::TT_X0), x(fr::TT_HI), x(fr::TT_X1));
        let mut mesh = egui::Mesh::default();
        let c = p.accent_fill.gamma_multiply(0.35);
        for (xx, col) in [(x0, Color32::TRANSPARENT), (xf, c), (x1, c)] {
            mesh.colored_vertex(pos2(xx, inner.top()), col);
            mesh.colored_vertex(pos2(xx, inner.bottom()), col);
        }
        for k in [0u32, 2] {
            mesh.add_triangle(k, k + 1, k + 2);
            mesh.add_triangle(k + 1, k + 2, k + 3);
        }
        painter.add(Shape::mesh(mesh));
    }
    let curve = |f: &dyn Fn(f64) -> f64, color: Color32, width: f32, dash: Option<(f32, f32)>| {
        let pts: Vec<Pos2> = (0..=(w as usize)).map(|i| {
            let fr_ = 20.0 * 1000f64.powf(i as f64 / w);
            pos2(inner.left() + i as f32, y(f(fr_)))
        }).collect();
        match dash {
            None => {
                painter.add(Shape::line(pts, Stroke::new(width, color)));
            }
            Some((d, g)) => {
                painter.extend(Shape::dashed_line(&pts, Stroke::new(width, color), d, g));
            }
        }
    };
    let raw = |f: f64| fr::interp(&meas.raw, f);
    curve(&raw, p.band_line, 1.5, prof.as_ref().map(|_| (2.0, 3.0))); // raw measurement (dotted when corrected)
    if prof.is_some() {
        curve(&measured, p.ink2, 1.5, None); // ear-corrected measurement
    }
    if let Some(t) = &tgt {
        if prof.is_some() {
            let rawt = |f: f64| fr::interp(&t.raw, f);
            curve(&rawt, p.band_line, 1.25, Some((2.0, 3.0))); // raw target, for reference
        }
        let tg = |f: f64| target(f).unwrap_or(0.0);
        curve(&tg, p.pos, 2.0, Some((6.0, 4.0))); // target (dashed): broad above ~6 kHz when corrected
    }
    curve(&result, p.accent, 2.5, None); // EQ'd result
    // legend
    let mut lx = inner.left() + 14.0;
    let ly = inner.top() + 14.0;
    let mut leg = |label: String, color: Color32, dash: Option<(f32, f32)>| {
        let a = pos2(lx, ly);
        let b = pos2(lx + 16.0, ly);
        match dash {
            None => {
                painter.line_segment([a, b], Stroke::new(2.5, color));
            }
            Some((d, g)) => {
                painter.extend(Shape::dashed_line(&[a, b], Stroke::new(2.5, color), d, g));
            }
        }
        let r = painter.text(pos2(lx + 21.0, ly), Align2::LEFT_CENTER, &label, mono(10.0), p.ink2);
        lx += 21.0 + r.width() + 16.0;
    };
    leg(if prof.is_some() { "rig".into() } else { "measurement".into() }, p.band_line, prof.as_ref().map(|_| (2.0, 3.0)));
    if prof.is_some() {
        let sel = app.frs.active_profile().map(|(n, _)| n.to_string()).unwrap_or_default();
        leg(format!("ear-corrected · {}", sel), p.ink2, None);
    }
    if tgt.is_some() {
        leg(if prof.is_some() { "target, broad above 6k".into() } else { "target".into() }, p.pos, Some((6.0, 4.0)));
    }
    leg("with EQ".into(), p.accent, None);
}

// ---- Table 1 ----
// Values are typed rather than dragged: a slider can't land on 8000 Hz or Q 0.71. Enter or
// leaving the field commits, Escape reverts, Up/Down nudge (Shift for bigger steps). Frequency
// takes "8000", "8k" or "8.5 kHz".

fn band_table(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    ui.spacing_mut().item_spacing.y = 0.0;
    let w = ui.available_width();
    let cols = [30.0f32, 58.0];
    let bar_w = (w * 0.32).max(120.0);
    let rest = w - cols[0] - cols[1] - bar_w;
    let field_w = rest / 3.0;
    let xs = [0.0, cols[0], cols[0] + cols[1], cols[0] + cols[1] + field_w, cols[0] + cols[1] + 2.0 * field_w, cols[0] + cols[1] + 3.0 * field_w];
    // head
    let (hr, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
    let pt = ui.painter();
    let hf = monos(10.0);
    let head = |pt: &egui::Painter, x: f32, t: &str, right: bool| {
        let g = pt.layout_no_wrap(t.to_uppercase(), hf.clone(), p.muted);
        let gx = if right { x - g.size().x - 8.0 } else { x };
        pt.galley(pos2(gx, hr.center().y - g.size().y / 2.0), g, p.muted);
    };
    head(pt, hr.left() + xs[0], "#", false);
    head(pt, hr.left() + xs[1] + 8.0, "Type", false);
    head(pt, hr.left() + xs[3], "Freq, Hz", true);
    head(pt, hr.left() + xs[4], "Gain, dB", true);
    head(pt, hr.left() + xs[5], "Q", true);
    let gs = app.gain_scale();
    let sc = format!("−{} · 0 · +{}", eq::js_num(gs), eq::js_num(gs));
    pt.text(pos2(hr.left() + xs[5] + 16.0 + (bar_w - 16.0) / 2.0, hr.center().y), Align2::CENTER_CENTER, sc, monos(10.0), p.muted);
    pt.line_segment([hr.left_bottom(), hr.right_bottom()], Stroke::new(1.0, p.grid_zero));
    let g_max = app.cur_g_max();
    for i in 0..app.model.len() {
        let (rr, _) = ui.allocate_exact_size(vec2(w, 33.0), Sense::hover());
        let b = app.model[i];
        let flat = b.gain == 0.0;
        let pt = ui.painter();
        pt.line_segment([rr.left_bottom(), rr.right_bottom()], Stroke::new(1.0, p.line));
        // number chip
        let chip = Rect::from_center_size(pos2(rr.left() + 10.0, rr.center().y), Vec2::splat(20.0));
        pt.rect_filled(chip, 6, if flat { p.node_dim } else { p.accent });
        pt.text(chip.center(), Align2::CENTER_CENTER, format!("{}", i + 1), monos(10.0), if flat { p.node_dim_ink } else { p.accent_ink });
        // type (click to cycle)
        let tr = Rect::from_min_size(pos2(rr.left() + xs[1] + 8.0, rr.top() + 3.0), vec2(40.0, 27.0));
        let tresp = ui.interact(tr, Id::new(("type", i)), Sense::click()).on_hover_text("Click to change filter type").on_hover_cursor(CursorIcon::PointingHand);
        let tc = if tresp.hovered() { p.accent } else { p.ink2 };
        ui.painter().text(pos2(tr.left(), tr.center().y), Align2::LEFT_CENTER, b.typ.label(), monos(10.0).clone(), tc);
        if tresp.clicked() {
            app.cycle_type(i);
        }
        // fields
        let sign = if b.gain == 0.0 { 0 } else if b.gain > 0.0 { 1 } else { -1 };
        for (k, field) in [Field::Freq, Field::Gain, Field::Q].into_iter().enumerate() {
            let fr_ = Rect::from_min_max(pos2(rr.left() + xs[2 + k] + 8.0, rr.top() + 2.0), pos2(rr.left() + xs[3 + k] - 8.0, rr.bottom() - 2.0));
            let color = if flat {
                p.faint
            } else if field == Field::Gain {
                match sign {
                    1 => p.pos,
                    -1 => p.accent,
                    _ => p.faint,
                }
            } else {
                p.ink
            };
            cell(app, ui, fr_, i, field, color, g_max);
        }
        // gain bar, centred on 0 dB, scaled to the cable's gain range
        let br = Rect::from_min_max(pos2(rr.left() + xs[5] + 16.0, rr.center().y - 5.0), pos2(rr.right(), rr.center().y + 5.0));
        let pt = ui.painter();
        let mid = br.center().x;
        pt.line_segment([pos2(mid, br.top() - 4.0), pos2(mid, br.bottom() + 4.0)], Stroke::new(1.0, p.grid_zero));
        let bw = (b.gain.abs() / app.gain_scale()).min(1.0) as f32 * br.width() / 2.0;
        if bw > 0.0 {
            let x0 = if b.gain >= 0.0 { mid } else { mid - bw };
            let c = if sign > 0 { p.pos } else if sign < 0 { p.accent } else { p.faint };
            pt.rect_filled(Rect::from_min_size(pos2(x0, br.top() + 2.0), vec2(bw, 6.0)), 2, c);
        }
    }
}

/// One typed value in the band table.
fn cell(app: &mut App, ui: &mut Ui, rect: Rect, i: usize, k: Field, color: Color32, g_max: f64) {
    let p = app.pal;
    let id = Id::new(("cell", i, k as u8));
    let te_id = id.with("te");
    let focused = ui.memory(|m| m.has_focus(te_id));
    // Up/Down nudge the value; taken before the text field sees them
    if focused {
        let (up, down, shift, esc, enter) = ui.input_mut(|inp| {
            let shift = inp.modifiers.shift;
            (
                inp.consume_key(egui::Modifiers::NONE, Key::ArrowUp) | inp.consume_key(egui::Modifiers::SHIFT, Key::ArrowUp),
                inp.consume_key(egui::Modifiers::NONE, Key::ArrowDown) | inp.consume_key(egui::Modifiers::SHIFT, Key::ArrowDown),
                shift,
                inp.consume_key(egui::Modifiers::NONE, Key::Escape),
                inp.key_pressed(Key::Enter),
            )
        });
        if up || down {
            if !app.edit_undone {
                app.push_undo();
                app.edit_undone = true;
            }
            let caps = app.caps.clone();
            eq::nudge_field(&mut app.model[i], k, if up { 1.0 } else { -1.0 }, shift, &caps, g_max);
            app.edit.insert((i, k), eq::fmt_field(k, field_val(&app.model[i], k)));
            select_all(ui, te_id);
        }
        if esc {
            app.edit.remove(&(i, k));
            ui.memory_mut(|m| m.surrender_focus(te_id));
            return;
        }
        let _ = enter;
    }
    let cur = eq::fmt_field(k, field_val(&app.model[i], k));
    let mut s = app.edit.get(&(i, k)).cloned().unwrap_or_else(|| cur.clone());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::right_to_left(Align::Center)));
    let tip = match k {
        Field::Freq => "Frequency (Hz, or e.g. 8k)",
        Field::Gain => "Gain (dB)",
        Field::Q => "Q",
    };
    let resp = child
        .add(
            egui::TextEdit::singleline(&mut s)
                .id(te_id)
                .frame(egui::Frame::NONE)
                .desired_width(rect.width())
                .font(monom(12.5))
                .text_color(color)
                .horizontal_align(Align::RIGHT)
                .margin(Margin::symmetric(0, 6)),
        )
        .on_hover_text(tip);
    if resp.has_focus() {
        // focus underline
        child.painter().line_segment([pos2(rect.left(), rect.bottom() - 1.0), pos2(rect.right(), rect.bottom() - 1.0)], Stroke::new(2.0, p.accent));
    }
    if resp.gained_focus() {
        app.edit_undone = false;
        select_all(&child, te_id);
    }
    if resp.has_focus() || resp.changed() {
        app.edit.insert((i, k), s.clone());
    }
    let enter = resp.has_focus() && child.input(|inp| inp.key_pressed(Key::Enter));
    if resp.lost_focus() || enter {
        commit(app, i, k, &s, g_max);
        if enter {
            app.edit.insert((i, k), eq::fmt_field(k, field_val(&app.model[i], k)));
            select_all(&child, te_id);
        } else {
            app.edit.remove(&(i, k));
            app.edit_undone = false;
        }
    }
}

fn select_all(ui: &Ui, id: Id) {
    if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), id) {
        let ccr = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(usize::MAX / 2));
        st.cursor.set_char_range(Some(ccr));
        st.store(ui.ctx(), id);
    }
}

fn field_val(b: &eq::Band, k: Field) -> f64 {
    match k {
        Field::Freq => b.freq,
        Field::Gain => b.gain,
        Field::Q => b.q,
    }
}

fn commit(app: &mut App, i: usize, k: Field, s: &str, g_max: f64) {
    let Some(v) = eq::parse_field(k, s) else { return };
    let caps = app.caps.clone();
    let mut probe = app.model[i];
    eq::set_field(&mut probe, k, v, &caps, g_max);
    if field_val(&probe, k) != field_val(&app.model[i], k) {
        if !app.edit_undone {
            app.push_undo();
            app.edit_undone = true;
        }
        app.model[i] = probe;
    }
}

// ---- the right-hand rail: the workflow, in order ----

fn rail(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    // 04 Write, always visible at the bottom
    egui::Panel::bottom("write")
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(p.shell).inner_margin(Margin { left: 0, right: 0, top: 14, bottom: 0 }))
        .show(ui, |ui| {
            ui.set_width(300.0);
            ui.spacing_mut().item_spacing.y = 7.0;
            step(ui, &p, "04", "Write", None);
            ui.horizontal(|ui| {
                caps(ui, &p, "Preamp");
                ui.add_space(10.0);
                preamp_field(app, ui);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if app.caps.gen == 1 && button(ui, &p, "+6 dB", if app.hi_gain { Kind::SmallOn } else { Kind::Small }, true).on_hover_text("Raise the per-band ceiling from +3 to +6 dB").clicked() {
                        app.toggle_hi_gain();
                    }
                    if button(ui, &p, "Auto headroom", Kind::Small, true).on_hover_text("Set preamp to just avoid clipping").clicked() {
                        app.push_undo();
                        let v = app.auto_preamp_value();
                        app.set_preamp(v);
                    }
                });
            });
            let (t, warn) = app.clip_text();
            theme::note(ui, &p, &t, warn, true);
            if let Some(n) = app.write_note.clone() {
                theme::note(ui, &p, &n, true, false);
            }
            let label = app.write_label();
            let enabled = !app.writing && !app.bank_busy;
            if theme::primary(ui, &p, &label, 300.0, 46.0, semi(14.0), enabled).clicked() {
                app.write_clicked();
            }
        });
    egui::CentralPanel::default().frame(egui::Frame::new().fill(p.shell)).show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.set_width(300.0);
            presets_card(app, ui);
            // 01 Measurement
            if step(ui, &p, "01", "Measurement", Some("FR files…")) {
                app.open_modal(Modal::Fr);
            }
            let rigs = ["5128", "711", "other"];
            let sel = rigs.iter().position(|r| *r == app.frs.rig).unwrap_or(0);
            if let Some(i) = theme::segmented(ui, &p, &rigs, sel, monos(11.0), vec2(14.0, 6.0), true) {
                app.set_fr_rig(rigs[i]);
            }
            meas_selects(app, ui);
            ui.add_space(6.0);
            // 02 Fit
            if step(ui, &p, "02", "Fit", Some("reset")) {
                app.fit = crate::fit::FitCfg { bands: app.caps.bands, ..Default::default() };
                app.save_settings();
            }
            fit_section(app, ui);
            ui.add_space(6.0);
            // 03 Treble, by ear
            if step(ui, &p, "03", "Treble, by ear", Some("profiles")) {
                app.open_treble();
            }
            treble_step(app, ui);
        });
    });
}

fn preamp_field(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let id = Id::new("preamp");
    let mut s = app.preamp_text.clone().unwrap_or_else(|| fixed(app.preamp, 1));
    ui.spacing_mut().item_spacing.x = 4.0;
    let resp = ui.add(egui::TextEdit::singleline(&mut s).id(id).frame(egui::Frame::NONE).desired_width(62.0).font(monom(16.0)).horizontal_align(Align::RIGHT).margin(Margin::symmetric(0, 2)));
    let r = resp.rect;
    let c = if resp.has_focus() { p.accent } else { p.line };
    ui.painter().extend(Shape::dashed_line(&[pos2(r.left(), r.bottom()), pos2(r.right(), r.bottom())], Stroke::new(1.0, c), 3.0, 2.0));
    text(ui, "dB", monom(16.0), p.ink);
    if resp.gained_focus() {
        select_all(ui, id);
    }
    if resp.has_focus() {
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            app.preamp_text = None;
            ui.memory_mut(|m| m.surrender_focus(id));
            return;
        }
        app.preamp_text = Some(s.clone());
    }
    if resp.lost_focus() {
        if let Some(v) = eq::parse_field(Field::Gain, &s) {
            if app.caps.pre_round(v) != app.preamp {
                app.push_undo();
                app.set_preamp(v);
            }
        }
        app.preamp_text = None;
    }
}

fn presets_card(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let offline_banks = app.offline.as_ref().map(|o| !o.target.user_slots.is_empty()).unwrap_or(false);
    let show = offline_banks || app.cable.as_ref().map(|c| c.has_presets == Some(true)).unwrap_or(false);
    if !show {
        return;
    }
    let title = if app.offline.is_some() {
        "Bank set"
    } else if app.bank_device() {
        "EQ bank"
    } else {
        "Cable voicing"
    };
    if step(ui, &p, "", title, if app.cur_target.is_some() { Some("banks…") } else { None }) {
        app.open_modal(Modal::Banks);
    }
    let active = app.active_bank;
    if app.offline.is_some() || app.bank_device() {
        // Only the banks you can write, plus bypass when a device is attached. The factory
        // voicings are left off; if the device happens to be on one, a line says so.
        let pm = app.presets_map();
        let users: Vec<u8> = app.user_slots_now().into_iter().filter(|s| pm.contains_key(s)).collect();
        let set_banks: Vec<u8> = if app.offline.is_some() { users.iter().copied().filter(|s| app.cur_set().map(|x| x.get(*s).is_some()).unwrap_or(false)).collect() } else { vec![] };
        let bw = (300.0 - 4.0 * 6.0) / 5.0;
        let busy = app.bank_busy;
        let mut clicked = None;
        egui::Grid::new("banks").spacing(vec2(6.0, 6.0)).show(ui, |ui| {
            for (k, s) in users.iter().enumerate() {
                let on = active == Some(*s as i32);
                let label = bank_short(&pm[s]);
                let resp = theme::button_sized(ui, &p, &label, vec2(bw, 32.0), on, !busy, monom(12.0));
                if set_banks.contains(s) {
                    ui.painter().circle_filled(pos2(resp.rect.right() - 7.0, resp.rect.top() + 6.0), 2.0, if on { p.accent_ink } else { p.accent });
                }
                let nm = app.bank_name(*s);
                let resp = resp.on_hover_text(if nm.is_empty() { pm[s].clone() } else { format!("{} · {}", pm[s], nm) });
                if resp.clicked() {
                    clicked = Some(*s);
                }
                if k % 5 == 4 {
                    ui.end_row();
                }
            }
        });
        if let Some(s) = clicked {
            if app.offline.is_some() {
                app.select_offline_bank(s);
            } else {
                app.switch_preset(s as i32);
            }
        }
        let by = if app.offline.is_some() { None } else { pm.iter().find(|(_, n)| is_off_slot(n)).map(|(k, _)| *k) };
        if let Some(by) = by {
            if theme::button_sized(ui, &p, &pm[&by], vec2(300.0, 32.0), active == Some(by as i32), !busy, monom(12.0)).on_hover_text("EQ off").clicked() {
                app.switch_preset(by as i32);
            }
        }
        let note = match active {
            Some(a) if a >= 0 && Some(a as u8) != by && !users.contains(&(a as u8)) => format!("On {} (factory). Pick a bank above to edit.", pm.get(&(a as u8)).cloned().unwrap_or_else(|| format!("bank {}", a))),
            Some(a) if a >= 0 && users.contains(&(a as u8)) => {
                let s = a as u8;
                let nm = app.bank_name(s);
                if app.offline.is_some() {
                    let has = app.cur_set().map(|x| x.get(s).is_some()).unwrap_or(false);
                    let sn = app.cur_set().map(|x| x.name.clone()).unwrap_or_default();
                    format!("{} · {} · set \"{}\"", pm[&s], if !nm.is_empty() { nm } else if has { "unnamed".into() } else { "empty".into() }, sn)
                } else if !nm.is_empty() {
                    format!("{} · {}", pm[&s], nm)
                } else {
                    String::new()
                }
            }
            _ => String::new(),
        };
        if !note.is_empty() {
            text(ui, note, sans(11.0), p.muted);
        }
    } else if let Some(c) = app.cable.clone() {
        for (i, name) in &c.presets {
            let on = active == Some(*i as i32);
            if theme::button_sized(ui, &p, name, vec2(300.0, 34.0), on, true, monom(12.0)).on_hover_text(format!("Preset {} · {}", i, name)).clicked() {
                app.switch_preset(*i as i32);
            }
        }
    }
    ui.add_space(6.0);
}

fn meas_selects(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let rig = app.frs.rig.clone();
    egui::Grid::new("measgrid").num_columns(2).spacing(vec2(10.0, 7.0)).min_col_width(64.0).show(ui, |ui| {
        caps(ui, &p, "IEM");
        let names: Vec<String> = app.frs.meas.iter().filter(|(_, r)| r.rig == rig).map(|(k, _)| k.clone()).collect();
        let cur = app.frs.cur_meas.clone().unwrap_or_else(|| if names.is_empty() { "(import a measurement)".into() } else { String::new() });
        let mut pick: Option<Option<String>> = None;
        let mut import = false;
        egui::ComboBox::from_id_salt("measSel").width(226.0).selected_text(egui::RichText::new(cur).font(mono(11.5)).color(p.ink2)).show_ui(ui, |ui| {
            for n in &names {
                if ui.selectable_label(app.frs.cur_meas.as_deref() == Some(n), egui::RichText::new(n).font(mono(11.5))).clicked() {
                    pick = Some(Some(n.clone()));
                }
            }
            if ui.selectable_label(false, egui::RichText::new("Import…").font(mono(11.5))).clicked() {
                import = true;
            }
        });
        ui.end_row();
        caps(ui, &p, "Target");
        let tg: Vec<String> = app.frs.targets.iter().filter(|(_, r)| r.rig == rig).map(|(k, _)| k.clone()).collect();
        let ms: Vec<String> = names.iter().filter(|n| Some(*n) != app.frs.cur_meas.as_ref()).cloned().collect();
        let cur_t = match &app.frs.cur_tgt {
            Some(t) => t.strip_prefix("t:").or_else(|| t.strip_prefix("m:")).unwrap_or(t).to_string(),
            None => "— no target —".into(),
        };
        let mut pick_t: Option<Option<String>> = None;
        egui::ComboBox::from_id_salt("tgtSel").width(226.0).selected_text(egui::RichText::new(cur_t).font(mono(11.5)).color(p.ink2)).show_ui(ui, |ui| {
            if ui.selectable_label(app.frs.cur_tgt.is_none(), egui::RichText::new("— no target —").font(mono(11.5))).clicked() {
                pick_t = Some(None);
            }
            if !tg.is_empty() {
                caps(ui, &p, "Targets");
                for n in &tg {
                    let v = format!("t:{}", n);
                    if ui.selectable_label(app.frs.cur_tgt.as_deref() == Some(&v), egui::RichText::new(n).font(mono(11.5))).clicked() {
                        pick_t = Some(Some(v));
                    }
                }
            }
            if !ms.is_empty() {
                caps(ui, &p, "Other measurements");
                for n in &ms {
                    let v = format!("m:{}", n);
                    if ui.selectable_label(app.frs.cur_tgt.as_deref() == Some(&v), egui::RichText::new(n).font(mono(11.5))).clicked() {
                        pick_t = Some(Some(v));
                    }
                }
            }
            if ui.selectable_label(false, egui::RichText::new("Import target…").font(mono(11.5))).clicked() {
                import = true;
            }
        });
        ui.end_row();
        if let Some(m) = pick {
            app.frs.cur_meas = m;
            app.persist_frs();
        }
        if let Some(t) = pick_t {
            app.frs.cur_tgt = t;
            app.persist_frs();
        }
        if import {
            app.open_modal(Modal::Fr);
        }
    });
}

fn fit_section(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let mut mode = app.fit.mode;
    ui.spacing_mut().item_spacing.y = 7.0;
    ui.radio_value(&mut mode, FitMode::Range, egui::RichText::new("Measurement only, up to a frequency").font(sans(13.0)).color(p.ink2));
    ui.radio_value(&mut mode, FitMode::Treble, egui::RichText::new("Whole target, treble from your ears").font(sans(13.0)).color(p.ink2));
    if mode != app.fit.mode {
        app.fit.mode = mode;
        app.save_settings();
    }
    let tm = app.fit.mode == FitMode::Treble;
    let nb = app.caps.bands;
    let mut changed = false;
    let row = |ui: &mut Ui, label: &str, tip: &str, id: &str, v: &mut f64| -> bool {
        ui.horizontal(|ui| {
            caps(ui, &p, label).on_hover_text(tip);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| num_field(ui, &p, Id::new(id), v, 96.0, 0)).inner
        })
        .inner
    };
    if !tm {
        let mut lo = app.fit.f_low;
        if row(ui, "Low bound, Hz", "", "fitLow", &mut lo) {
            app.fit.f_low = eq::clamp(eq::jround(lo), 20.0, 20000.0);
            changed = true;
        }
        let mut hi = app.fit.f_high;
        if row(ui, "High bound, Hz", "", "fitHigh", &mut hi) {
            app.fit.f_high = eq::clamp(eq::jround(hi), 20.0, 20000.0);
            changed = true;
        }
    } else {
        let mut top = app.fit.top;
        if row(ui, "Upper limit, Hz", "Nothing is fitted above this: set it to where your hearing stops", "fitTop", &mut top) {
            app.fit.top = eq::clamp(eq::jround(top), 8000.0, 20000.0);
            changed = true;
        }
    }
    let mut b = app.fit.bands as f64;
    if row(ui, &format!("Bands of {}", nb), "", "fitBands", &mut b) {
        app.fit.bands = (eq::jround(b).max(1.0) as usize).clamp(1, nb);
        changed = true;
    }
    if changed {
        app.save_settings();
    }
    // the mode's note
    if !tm {
        para(ui, "Outside the bounds the EQ is kept flat; bands the fit doesn't need stay free.", sans(11.5), p.muted);
    } else if let Some((n, _)) = app.frs.active_profile() {
        para(ui, format!("Above ~6 kHz the fit follows the target's broad shape and your {} profile, not the rig's treble detail.", n), sans(11.5), p.muted);
    } else {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            text(ui, "Needs a treble profile for this measurement (step 03). ", sans(11.5), p.muted);
            if link(ui, &p, "Run the test…", sans(11.5), p.accent).clicked() {
                app.open_treble();
            }
        });
    }
    if button(ui, &p, "Auto-fit", Kind::On, true).on_hover_text(format!("Auto-fit the {} bands to your target", nb)).clicked() {
        app.auto_fit_clicked();
    }
}

fn treble_step(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let prof = app.frs.active_profile().map(|(n, pr)| (n.to_string(), pr.date.clone(), pr.bands.clone()));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        match &prof {
            Some((n, d, _)) => {
                text(ui, n, semi(13.0), p.ink);
                text(ui, format!(" · {}", d), sans(13.0), p.muted);
            }
            None => {
                text(ui, if app.frs.meas_rec().is_some() { "No profile for this IEM yet" } else { "No measurement selected" }, sans(13.0), p.muted);
            }
        }
    });
    ui.horizontal(|ui| {
        if button(ui, &p, "Run test", Kind::Normal, true).on_hover_text("Run the treble listening test").clicked() {
            app.open_treble();
        }
        if button(ui, &p, "Apply to EQ", Kind::Normal, prof.is_some()).on_hover_text("Turn the selected profile into treble bands in the EQ's flat slots").clicked() {
            if let Some((_, _, b)) = &prof {
                let (msg, ok) = app.apply_treble_profile(b);
                app.treble_msg = Some((msg, ok, std::time::Instant::now() + std::time::Duration::from_secs(6)));
            }
        }
    });
    if let Some((m, ok, until)) = app.treble_msg.clone() {
        if until > std::time::Instant::now() {
            theme::note(ui, &p, &m, !ok, false);
        } else {
            app.treble_msg = None;
        }
    }
}

// ---- toast, file drop, window edges ----

fn overlays(app: &mut App, ctx: &egui::Context) {
    let p = app.pal;
    let screen = ctx.content_rect();
    if app.hovering_files {
        egui::Area::new(Id::new("dropzone")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0, Color32::from_black_alpha(90));
            let r = Rect::from_center_size(screen.center(), vec2(440.0, 110.0));
            ui.painter().rect_filled(r, 16, p.card);
            ui.painter().extend(dashed_rect(r.shrink(1.0), Stroke::new(2.0, p.accent)));
            ui.painter().text(r.center() - vec2(0.0, 18.0), Align2::CENTER_CENTER, "Drop an EQ or FR file", semi(17.0), p.ink);
            let g = ui.painter().layout(
                "A squig / AutoEQ parametric EQ is applied to the current EQ. A frequency response is added as a source or target.".into(),
                sans(12.5),
                p.muted,
                380.0,
            );
            ui.painter().galley(pos2(r.center().x - g.size().x / 2.0, r.center().y + 2.0), g, p.muted);
        });
    }
    if let Some((msg, warn, until)) = app.toast.clone() {
        if until <= std::time::Instant::now() {
            app.toast = None;
        } else {
            egui::Area::new(Id::new("toast")).order(egui::Order::Tooltip).anchor(Align2::CENTER_BOTTOM, vec2(0.0, -22.0)).show(ctx, |ui| {
                card_frame(&p, true).stroke(Stroke::new(1.0, if warn { p.danger } else { p.line })).corner_radius(10).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                    ui.set_max_width(640.0f32.min(screen.width() - 48.0));
                    para(ui, msg, sans(12.5), p.ink);
                });
            });
        }
    }
}

fn dashed_rect(r: Rect, st: Stroke) -> Vec<Shape> {
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    Shape::dashed_line(&pts, st, 8.0, 6.0)
}

/// The frameless window has no system border, so its edges are made resizable here.
fn resize_edges(ctx: &egui::Context) {
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return;
    }
    use egui::viewport::ResizeDirection as D;
    let r = ctx.content_rect();
    let e = 5.0;
    let zones = [
        (Rect::from_min_max(r.left_top(), pos2(r.left() + e * 2.0, r.top() + e * 2.0)), D::NorthWest, CursorIcon::ResizeNorthWest),
        (Rect::from_min_max(pos2(r.right() - e * 2.0, r.top()), pos2(r.right(), r.top() + e * 2.0)), D::NorthEast, CursorIcon::ResizeNorthEast),
        (Rect::from_min_max(pos2(r.left(), r.bottom() - e * 2.0), pos2(r.left() + e * 2.0, r.bottom())), D::SouthWest, CursorIcon::ResizeSouthWest),
        (Rect::from_min_max(pos2(r.right() - e * 2.0, r.bottom() - e * 2.0), r.right_bottom()), D::SouthEast, CursorIcon::ResizeSouthEast),
        (Rect::from_min_max(r.left_top(), pos2(r.right(), r.top() + e)), D::North, CursorIcon::ResizeNorth),
        (Rect::from_min_max(pos2(r.left(), r.bottom() - e), r.right_bottom()), D::South, CursorIcon::ResizeSouth),
        (Rect::from_min_max(r.left_top(), pos2(r.left() + e, r.bottom())), D::West, CursorIcon::ResizeWest),
        (Rect::from_min_max(pos2(r.right() - e, r.top()), r.right_bottom()), D::East, CursorIcon::ResizeEast),
    ];
    egui::Area::new(Id::new("edges")).order(egui::Order::Foreground).fixed_pos(r.min).interactable(true).show(ctx, |ui| {
        for (k, (z, d, c)) in zones.into_iter().enumerate() {
            let resp = ui.interact(z, Id::new(("edge", k)), Sense::drag());
            if resp.hovered() {
                ctx.set_cursor_icon(c);
            }
            if resp.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(d));
            }
        }
    });
}
