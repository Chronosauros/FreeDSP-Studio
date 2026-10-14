// The dialogs: EQ as text, backup, saved EQs, bank sets, a dropped curve, the treble listening
// test, the log, the clip warning, about, and measurement & target files.

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, Id, Key, Layout, Margin, Rect, Sense, Stroke, Ui};

use crate::app::{bank_summary, clips, App, Modal, TtView, STARTING};
use crate::eq::{self, fixed};
use crate::fr;
use crate::theme::{self, button, caps, link, med, mono, monos, para, sans, semi, text, Kind, Pal, WinIcon};
use crate::treble::{Loudness, Method};
use crate::ui::card_frame;

pub fn draw(app: &mut App, ctx: &egui::Context) {
    let Some(m) = app.modal else { return };
    let p = app.pal;
    let width: f32 = match m {
        Modal::Banks => 720.0,
        _ => 520.0,
    };
    let max_h = ctx.content_rect().height() - 96.0;
    let running = app.tt.session.is_some();
    let resp = egui::Modal::new(Id::new(("modal", m as u8)))
        .backdrop_color(Color32::from_black_alpha(115))
        .frame(card_frame(&p, true).inner_margin(Margin::symmetric(20, 18)))
        .show(ctx, |ui| {
            ui.set_width(width.min(ctx.content_rect().width() - 48.0) - 40.0);
            ui.set_max_height(max_h);
            ui.spacing_mut().item_spacing.y = 12.0;
            match m {
                Modal::EqText => eq_text(app, ui),
                Modal::Backup => backup(app, ui),
                Modal::Saved => saved(app, ui),
                Modal::Banks => banks(app, ui),
                Modal::Drop => dropped(app, ui),
                Modal::Treble => treble(app, ui),
                Modal::Log => log(app, ui),
                Modal::Clip => clip(app, ui),
                Modal::About => about(app, ui),
                Modal::Fr => frs(app, ui),
            }
        });
    // A stray click or Escape mustn't end a running test; Stop does that.
    if resp.should_close() && !running && app.tt.result.is_none() && app.modal == Some(m) {
        app.close_modal();
    }
}

/// Title, subtitle and the close cross. Returns true if closed.
fn head(app: &mut App, ui: &mut Ui, title: &str, sub: &str) -> bool {
    let p = app.pal;
    let mut close = false;
    ui.horizontal_top(|ui| {
        let w = ui.available_width() - 34.0;
        ui.allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            text(ui, title, semi(15.0), p.ink);
            if !sub.is_empty() {
                para(ui, sub, sans(12.0), p.muted);
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            if theme::win_button(ui, &p, WinIcon::Close, false).on_hover_text("Close").clicked() {
                close = true;
            }
        });
    });
    if close {
        app.close_modal();
    }
    close
}

fn right_row(ui: &mut Ui, f: impl FnOnce(&mut Ui)) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        f(ui)
    });
}

// ---- EQ as text ----

fn eq_text(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    if head(app, ui, "EQ as text", "This mirrors the current EQ as squig / Equalizer APO parametric text. Paste a squig export or type it in, then Apply.") {
        return;
    }
    ui.add(
        egui::TextEdit::multiline(&mut app.eq_text)
            .font(mono(12.0))
            .desired_width(f32::INFINITY)
            .desired_rows(9)
            .hint_text("Preamp: -3.8 dB\nFilter 1: ON PK Fc 105 Hz Gain 3.0 dB Q 0.70\nFilter 2: ON LSC Fc 60 Hz Gain 4.0 dB Q 0.70 ...")
            .margin(Margin::same(10)),
    );
    ui.horizontal(|ui| {
        let w = ui.available_width() - 200.0;
        ui.allocate_ui(vec2(w, 0.0), |ui| {
            para(ui, &app.eq_text_msg, sans(12.0), p.muted);
        });
        right_row(ui, |ui| {
            if button(ui, &p, "Apply to EQ", Kind::On, true).clicked() {
                let parsed = eq::parse_squig(&app.eq_text);
                app.eq_text_msg = if parsed.filters.is_empty() { "No enabled \"Filter N: ON ...\" lines found.".into() } else { app.apply_parsed(&parsed) };
            }
            let copied = app.copied.map(|t| t > std::time::Instant::now()).unwrap_or(false);
            if button(ui, &p, if copied { "Copied" } else { "Copy" }, Kind::Normal, true).clicked() {
                ui.ctx().copy_text(app.eq_text.clone());
                app.copied = Some(std::time::Instant::now() + std::time::Duration::from_millis(1100));
            }
        });
    });
}

// ---- backup ----

fn backup(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    if head(app, ui, "Backup", "Save all your saved EQs and FR curves to one file, or restore from one. Handy for moving between machines.") {
        return;
    }
    para(ui, "Back up saves all your EQs, bank sets and FR curves to a .json file wherever you choose. Restore merges a backup file back in (nothing is overwritten unless the names match).", sans(12.5), p.muted);
    if let Some((m, until)) = app.backup_msg.clone() {
        if until > std::time::Instant::now() {
            text(ui, m, mono(11.0), p.pos);
        }
    }
    right_row(ui, |ui| {
        if button(ui, &p, "Back up now", Kind::On, true).clicked() {
            app.export_backup();
        }
        if button(ui, &p, "Restore…", Kind::Normal, true).clicked() {
            app.restore_backup();
        }
    });
}

// ---- saved EQs ----

fn saved(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    if head(app, ui, "Saved EQs", "Click one to load it into the editor. The built-in DUSK starting points come first.") {
        return;
    }
    let mut load: Option<Result<usize, String>> = None;
    let mut del: Option<String> = None;
    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        for (i, (n, bands)) in STARTING.iter().enumerate() {
            let nb = bands.iter().filter(|b| b.1 != 0.0).count();
            if theme::list_row(ui, &p, n, Some("built-in"), &format!("{} bands · 0.0", nb), false, false).0 {
                load = Some(Ok(i));
            }
        }
        let names: Vec<String> = app.saved.keys().cloned().collect();
        for n in names {
            let e = &app.saved[&n];
            let meta = format!("{} bands · {}", eq::active_count(&e.bands), fixed(e.preamp, 1));
            let (c, d) = theme::list_row(ui, &p, &n, None, &meta, false, true);
            if c {
                load = Some(Err(n.clone()));
            }
            if d {
                del = Some(n);
            }
        }
    });
    if let Some(n) = del {
        app.saved.shift_remove(&n);
        app.persist_saved();
    }
    if let Some(l) = load {
        match l {
            Ok(i) => app.load_starting(i),
            Err(n) => app.load_saved(&n),
        }
        app.close_modal();
        return;
    }
    ui.horizontal(|ui| {
        let r = theme::field(ui, &p, &mut app.save_name, ui.available_width() - 70.0, "Name the current EQ…", sans(12.0));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if button(ui, &p, "Save", Kind::On, true).clicked() || enter {
            if !app.save_current() {
                r.request_focus();
            }
        }
    });
}

// ---- bank sets ----

fn banks(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let Some(t) = app.cur_target.clone() else {
        app.close_modal();
        return;
    };
    let Some(set_id) = app.cur_set_id() else { return };
    let sub = if app.offline.is_some() {
        format!("{}, designing offline. Store puts the editor's EQ in a bank, Load brings one back; drag a bank by its handle to move it. Plug the device in to write the set.", t.name)
    } else {
        format!(
            "{}. Store puts the editor's EQ in a bank, Send writes one bank to the device now, Write all writes every filled bank. Drag a bank by its handle to move it.",
            app.cable.as_ref().map(|c| c.name.clone()).unwrap_or_default()
        )
    };
    if head(app, ui, "Bank set", &sub) {
        return;
    }
    let busy = app.bank_busy;
    // the set row
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = ui.available_width() - 210.0;
        if let Some(name) = app.set_rename.as_mut() {
            let r = ui.add(egui::TextEdit::singleline(name).desired_width(w).font(sans(12.0)).char_limit(60).margin(Margin::symmetric(8, 7)));
            r.request_focus();
            let esc = ui.input(|i| i.key_pressed(Key::Escape));
            if r.lost_focus() || esc {
                let v = name.trim().to_string();
                if !esc && !v.is_empty() {
                    if let Some(s) = app.sets.sets.get_mut(&set_id) {
                        s.name = v;
                    }
                    app.persist_sets();
                }
                app.set_rename = None;
            }
        } else {
            let ids = app.sets_for(&t.id);
            let cur_name = app.sets.sets.get(&set_id).map(|s| s.name.clone()).unwrap_or_default();
            let mut pick = None;
            ui.add_enabled_ui(!busy, |ui| {
                egui::ComboBox::from_id_salt("setSel").width(w).selected_text(egui::RichText::new(cur_name).font(sans(12.0))).show_ui(ui, |ui| {
                    for id in &ids {
                        let n = app.sets.sets[id].name.clone();
                        if ui.selectable_label(*id == set_id, egui::RichText::new(n).font(sans(12.0))).clicked() {
                            pick = Some(id.clone());
                        }
                    }
                });
            });
            if let Some(id) = pick {
                app.sets.current.insert(t.id.clone(), id);
                app.persist_sets();
                app.bank_msg.0.clear();
                if app.offline.is_some() {
                    if let Some(s) = app.sel_bank {
                        if app.cur_set().map(|x| x.get(s).is_some()).unwrap_or(false) {
                            app.load_bank_into_editor(s);
                        }
                    }
                }
            }
        }
        if button(ui, &p, "New", Kind::Small, !busy).on_hover_text("Start an empty set").clicked() {
            let n = format!("Set {}", app.sets_for(&t.id).len() + 1);
            app.create_set(&t.id, &n, Default::default(), true);
            app.bank_msg.0.clear();
            app.set_rename = Some(n);
        }
        if button(ui, &p, "Rename", Kind::Small, !busy).on_hover_text("Rename this set").clicked() {
            app.set_rename = app.sets.sets.get(&set_id).map(|s| s.name.clone());
        }
        let armed = matches!(&app.del_armed, Some((i, u)) if *i == set_id && *u > std::time::Instant::now());
        if button(ui, &p, if armed { "Sure?" } else { "Delete" }, Kind::Small, !busy).on_hover_text("Delete this set").clicked() {
            app.delete_set();
        }
    });
    // the banks
    let saved: Vec<String> = app.saved.keys().cloned().collect();
    let live = if app.offline.is_some() { app.sel_bank.map(|s| s as i32) } else { app.active_bank };
    let max_h = (ui.ctx().content_rect().height() - 330.0).max(160.0);
    let mut act: Option<(&'static str, u8, String)> = None;
    let pointer = ui.ctx().pointer_interact_pos();
    let mut drop_target: Option<u8> = None;
    egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        for s in t.user_slots.clone() {
            let bank = app.sets.sets.get(&set_id).and_then(|x| x.get(s)).cloned();
            let label = app.user_label(s);
            let w = ui.available_width();
            let (rr, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
            let over = app.bank_drag.is_some() && app.bank_drag != Some(s) && pointer.map(|q| rr.contains(q)).unwrap_or(false);
            if over {
                drop_target = Some(s);
            }
            ui.painter().rect(rr, 8, p.row, Stroke::new(1.0, if over { p.accent } else { Color32::TRANSPARENT }), egui::StrokeKind::Inside);
            if live == Some(s as i32) {
                ui.painter().rect_filled(Rect::from_min_size(rr.min, vec2(3.0, rr.height())), 0, p.accent);
            }
            let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rr.shrink2(vec2(8.0, 4.0))).layout(Layout::left_to_right(Align::Center)));
            row.spacing_mut().item_spacing.x = 8.0;
            // grip
            let (gr, gresp) = row.allocate_exact_size(vec2(14.0, 26.0), Sense::drag());
            for k in 0..3 {
                for c in 0..2 {
                    row.painter().circle_filled(pos2(gr.left() + 4.0 + c as f32 * 5.0, gr.center().y - 5.0 + k as f32 * 5.0), 1.2, p.faint);
                }
            }
            let gresp = gresp.on_hover_text("Drag to move this bank").on_hover_cursor(egui::CursorIcon::Grab);
            if gresp.drag_started() && !busy {
                app.bank_drag = Some(s);
            }
            row.allocate_ui_with_layout(vec2(52.0, 26.0), Layout::left_to_right(Align::Center), |ui| {
                text(ui, &label, med(11.5), p.ink2);
            });
            // name
            let mut nm = bank.as_ref().map(|b| b.name.clone()).unwrap_or_default();
            let r = row.add_enabled(
                bank.is_some() && !busy,
                egui::TextEdit::singleline(&mut nm).desired_width(150.0).font(sans(12.0)).char_limit(40).hint_text(if bank.is_some() { "name this EQ" } else { "empty" }).margin(Margin::symmetric(8, 5)),
            );
            if r.changed() {
                if let Some(b) = app.sets.sets.get_mut(&set_id).and_then(|x| x.get_mut(s)) {
                    b.name = nm.trim_start().to_string();
                }
            }
            if r.lost_focus() {
                if let Some(b) = app.sets.sets.get_mut(&set_id).and_then(|x| x.get_mut(s)) {
                    b.name = b.name.trim().to_string();
                }
                app.persist_sets();
            }
            let cl = clips(bank.as_ref());
            row.allocate_ui_with_layout(vec2(140.0, 26.0), Layout::left_to_right(Align::Center), |ui| {
                text(ui, format!("{}{}", bank_summary(bank.as_ref()), if cl { " · clips" } else { "" }), mono(10.5), if cl { p.danger } else { p.faint });
            });
            row.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if button(ui, &p, "✕", Kind::Small, bank.is_some() && !busy).on_hover_text("Empty this bank (the device is not touched)").clicked() {
                    act = Some(("empty", s, String::new()));
                }
                if app.offline.is_none() && button(ui, &p, "Send", Kind::Small, bank.is_some() && !busy).on_hover_text("Write this bank to the device now and switch to it").clicked() {
                    act = Some(("send", s, String::new()));
                }
                ui.add_enabled_ui(!saved.is_empty() && !busy, |ui| {
                    egui::ComboBox::from_id_salt(("fill", s)).width(66.0).selected_text(egui::RichText::new("Saved…").font(sans(11.5))).show_ui(ui, |ui| {
                        for n in &saved {
                            if ui.selectable_label(false, egui::RichText::new(n).font(sans(12.0))).clicked() {
                                act = Some(("fill", s, n.clone()));
                            }
                        }
                    })
                    .response
                    .on_hover_text("Fill this bank from a saved EQ");
                });
                if button(ui, &p, "Store", Kind::Small, !busy).on_hover_text("Put the editor's EQ in this bank").clicked() {
                    act = Some(("store", s, String::new()));
                }
                if button(ui, &p, "Load", Kind::Small, bank.is_some() && !busy).on_hover_text("Load this bank into the editor").clicked() {
                    act = Some(("load", s, String::new()));
                }
            });
        }
    });
    if let Some(from) = app.bank_drag {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if ui.input(|i| i.pointer.any_released()) {
            app.bank_drag = None;
            if let Some(to) = drop_target {
                app.move_bank(from, to);
            }
        }
    }
    if let Some((a, s, n)) = act {
        match a {
            "load" => app.load_bank_into_editor(s),
            "store" => app.store_into_bank(s),
            "fill" => app.fill_from_saved(s, &n),
            "send" => app.write_banks(Some(s)),
            _ => {
                if let Some(x) = app.sets.sets.get_mut(&set_id) {
                    x.remove(s);
                }
                app.persist_sets();
                let l = app.user_label(s);
                app.bank_msg(format!("Emptied {} in this set.", l), false);
            }
        }
    }
    let (m, warn) = app.bank_msg.clone();
    para(ui, m, sans(12.0), if warn { p.danger } else { p.muted });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if button(ui, &p, "Sort by name", Kind::Small, !busy).on_hover_text("Order filled banks by name, empty ones last").clicked() {
            app.sort_banks();
        }
        right_row(ui, |ui| {
            if app.offline.is_none() {
                let n = t.user_slots.iter().filter(|s| app.sets.sets.get(&set_id).map(|x| x.get(**s).is_some()).unwrap_or(false)).count();
                if button(ui, &p, &format!("Write {} bank{} to device", n, if n == 1 { "" } else { "s" }), Kind::SmallOn, !busy).on_hover_text("Write every filled bank to the device").clicked() {
                    app.write_banks(None);
                }
                if button(ui, &p, "Read all from device", Kind::Small, !busy).on_hover_text("Read every USER bank on the device into a new set").clicked() {
                    app.read_all_banks();
                }
            }
            if busy && button(ui, &p, "Stop", Kind::Small, true).clicked() {
                app.worker.stop.store(true, std::sync::atomic::Ordering::SeqCst);
                app.bank_msg("Stopping after this bank…", false);
            }
        });
    });
}

// ---- a dropped curve ----

fn dropped(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let Some(c) = app.drop_cur.as_ref() else {
        app.modal = None;
        return;
    };
    let more = app.drop_queue.len();
    let how = if c.channels > 1 {
        format!("Average of {} channels ({}), as squig plots it.", c.channels, c.file)
    } else {
        format!("From {}, {} points.{}", c.file, c.points.len(), if c.single.is_some() { " One channel only; drop both together to get the average squig shows." } else { "" })
    };
    let sub = format!("{}{}", how, if more > 0 { format!(" {} more after this.", more) } else { String::new() });
    if head(app, ui, "Add frequency response", &sub) {
        return;
    }
    let mut add = false;
    egui::Grid::new("dropform").num_columns(2).spacing(vec2(12.0, 10.0)).show(ui, |ui| {
        let c = app.drop_cur.as_mut().unwrap();
        caps(ui, &p, "Name");
        let r = ui.add(egui::TextEdit::singleline(&mut c.name).desired_width(340.0).font(sans(12.0)).char_limit(80).margin(Margin::symmetric(8, 7)));
        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            add = true;
        }
        ui.end_row();
        caps(ui, &p, "Rig");
        let rigs = ["5128", "711", "other"];
        let sel = rigs.iter().position(|r| *r == c.rig).unwrap_or(1);
        if let Some(i) = theme::segmented(ui, &p, &rigs, sel, monos(11.0), vec2(14.0, 6.0), true) {
            c.rig = rigs[i].into();
        }
        ui.end_row();
        caps(ui, &p, "It's a");
        if let Some(i) = theme::segmented(ui, &p, &["Source", "Target"], if c.meas { 0 } else { 1 }, monos(11.0), vec2(14.0, 6.0), true) {
            c.meas = i == 0;
        }
        ui.end_row();
    });
    let c = app.drop_cur.as_ref().unwrap();
    let name = c.name.trim().to_string();
    let exists = if c.meas { app.frs.meas.contains_key(&name) } else { app.frs.targets.contains_key(&name) };
    let msg = if name.is_empty() {
        "Give it a name.".to_string()
    } else if exists {
        format!("Replaces the existing {} \"{}\".", if c.meas { "source" } else { "target" }, name)
    } else {
        String::new()
    };
    para(ui, msg, sans(12.0), p.muted);
    right_row(ui, |ui| {
        if button(ui, &p, "Add", Kind::On, true).clicked() {
            add = true;
        }
        if more > 0 && button(ui, &p, "Skip", Kind::Normal, true).clicked() {
            app.next_drop();
        }
    });
    if add {
        app.add_dropped();
    }
}

// ---- treble listening test ----

fn treble(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let sub = match &app.frs.cur_meas {
        Some(m) if app.frs.meas_rec().is_some() => format!("Personal treble for \"{}\", above ~6 kHz.", m),
        _ => "Personal treble above ~6 kHz. No measurement selected: results can be applied to the EQ, not saved.".into(),
    };
    if head(app, ui, "Treble listening test", &sub) {
        return;
    }
    match app.tt.view {
        TtView::Setup => tt_setup(app, ui),
        TtView::Run => tt_run(app, ui),
        TtView::Done => tt_done(app, ui),
    }
    let _ = p;
}

fn tt_text(ui: &mut Ui, p: &Pal, bold: &str, rest: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if !bold.is_empty() {
            text(ui, bold, semi(12.5), p.ink2);
        }
        let mut job = egui::text::LayoutJob::default();
        job.append(rest, 0.0, egui::TextFormat { font_id: sans(12.5), color: p.muted, ..Default::default() });
        job.wrap.max_width = ui.available_width();
        ui.label(job);
    });
}

fn tt_setup(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(vec2(90.0, 28.0), Layout::left_to_right(Align::Center), |ui| {
            text(ui, "Method", sans(13.0), p.ink2);
        });
        let sel = if app.tt.method == Method::Noise { 0 } else { 1 };
        if let Some(i) = theme::segmented(ui, &p, &["Stands out", "Louder / quieter"], sel, monos(11.0), vec2(14.0, 6.0), true) {
            app.tt.method = if i == 0 { Method::Noise } else { Method::Match };
            app.save_settings();
        }
    });
    if app.tt.method == Method::Noise {
        tt_text(ui, &p, "", "You hear one treble band on its own, then pink noise with that band nudged up or down, and say whether it sticks out.");
    } else {
        tt_text(ui, &p, "", "You hear two short bursts of noise, each one treble band, and say whether the second is quieter, louder or the same. Each band is matched against its neighbours.");
    }
    tt_text(ui, &p, "Before you start: ", "cable as system output, normal listening volume, quiet room. If you EQ from a measurement, fit and write it up to ~6 kHz first.");
    tt_text(ui, &p, "Afterwards: ", "Apply to EQ puts the result straight into the EQ's flat bands, or save it as a profile for the TREBLE view and whole-target auto-fit.");
    if let Some(m) = app.frs.cur_meas.clone().filter(|m| app.frs.meas.contains_key(m)) {
        let tr = app.frs.meas[&m].treble.clone().unwrap_or_default();
        let sel_prof = tr.sel.as_ref().and_then(|s| tr.profiles.get(s)).cloned();
        ui.horizontal(|ui| {
            caps(ui, &p, "Profiles for this measurement");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let c = if sel_prof.is_some() { p.accent } else { p.faint };
                if link(ui, &p, "Apply selected to EQ", mono(11.0), c).on_hover_text("Turn the selected profile into treble bands on the current EQ").clicked() {
                    if let Some(pr) = &sel_prof {
                        let (msg, ok) = app.apply_treble_profile(&pr.bands);
                        app.tt.warn = msg;
                        app.tt.warn_ok = ok;
                    }
                }
            });
        });
        egui::ScrollArea::vertical().max_height(150.0).id_salt("ttprof").show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            if tr.profiles.is_empty() {
                text(ui, "None yet. Run a test to make one.", sans(12.0), p.faint);
            }
            let mut pick = None;
            let mut del = None;
            for (n, pr) in &tr.profiles {
                let meta = format!("{} · {}", pr.date, if pr.passes == 2 { "thorough" } else { "quick" });
                let (c, d) = theme::list_row(ui, &p, n, None, &meta, tr.sel.as_deref() == Some(n), true);
                if c {
                    pick = Some(n.clone());
                }
                if d {
                    del = Some(n.clone());
                }
            }
            if pick.is_some() || del.is_some() {
                if let Some(t) = app.frs.meas.get_mut(&m).and_then(|r| r.treble.as_mut()) {
                    if let Some(n) = pick {
                        t.sel = Some(n);
                    }
                    if let Some(n) = del {
                        t.profiles.shift_remove(&n);
                        if t.sel.as_deref() == Some(&n) {
                            t.sel = t.profiles.keys().next().cloned();
                        }
                    }
                }
                app.persist_frs();
            }
        });
    }
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(vec2(90.0, 28.0), Layout::left_to_right(Align::Center), |ui| {
            text(ui, "Tip", sans(13.0), p.ink2);
        });
        ui.add(egui::TextEdit::singleline(&mut app.tt.tip).desired_width(ui.available_width()).hint_text("e.g. Spinfit W1 M").font(sans(12.5)).margin(Margin::symmetric(10, 8)));
    });
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(vec2(90.0, 28.0), Layout::left_to_right(Align::Center), |ui| {
            text(ui, "Length", sans(13.0), p.ink2);
        });
        let (q, t) = if app.tt.method == Method::Match { (2, 4) } else { (3, 7) };
        let items = [format!("Quick · ~{} min", q), format!("Thorough · ~{} min", t)];
        let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
        if let Some(i) = theme::segmented(ui, &p, &refs, if app.tt.thorough { 1 } else { 0 }, monos(11.0), vec2(14.0, 6.0), true) {
            app.tt.thorough = i == 1;
        }
    });
    if !app.tt.warn.is_empty() {
        let w = app.tt.warn.clone();
        theme::note(ui, &p, &w, !app.tt.warn_ok, false);
    }
    ui.horizontal(|ui| {
        if button(ui, &p, if app.tt.level_on { "■ Stop" } else { "▶ Level check" }, Kind::Normal, true).clicked() {
            app.tt_level_toggle();
        }
        right_row(ui, |ui| {
            if button(ui, &p, "Start test", Kind::On, true).clicked() {
                app.tt_start();
            }
            let differs = app.cable.is_some() && !app.cable_eq.as_ref().map(|c| eq::same_eq(c, &app.model)).unwrap_or(false);
            if differs && button(ui, &p, "Write EQ to cable", Kind::Normal, !app.writing).clicked() {
                if app.eff_peak().0 > 0.1 {
                    app.tt.warn = "This EQ would clip. Set the preamp (Auto headroom) and try again.".into();
                    app.tt.warn_ok = false;
                } else {
                    app.do_write();
                }
            }
        });
    });
}

fn tt_run(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    // nothing here takes typing, so keys go to the test (the tip field keeps focus otherwise)
    ui.ctx().memory_mut(|m| {
        if let Some(f) = m.focused() {
            m.surrender_focus(f);
        }
    });
    tt_keys(app, ui);
    let Some(s) = app.tt.session.as_ref() else {
        app.tt.view = TtView::Setup;
        return;
    };
    let (prog, method, asking, paused, reseat, can_undo) = (s.progress(), s.method, s.asking, s.paused, s.wait_reseat, !s.undo.is_empty());
    let m = method == Method::Match;
    // progress
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
    ui.painter().rect_filled(r, 2, p.track);
    ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(r.width() * prog as f32, 4.0)), 2, p.accent);
    // stage
    egui::Frame::new().fill(p.row).corner_radius(12).inner_margin(Margin::symmetric(12, 18)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            caps(ui, &p, &app.tt.count.clone());
            text(ui, &app.tt.freq.clone(), monos(26.0), p.ink);
            let (ph, ask) = (app.tt.phase.clone(), app.tt.phase_ask);
            ui.allocate_ui(vec2(ui.available_width(), 18.0), |ui| {
                ui.vertical_centered(|ui| {
                    text(ui, ph, if ask { semi(12.5) } else { sans(12.5) }, if ask { p.accent } else { p.muted });
                });
            });
        });
    });
    if reseat {
        tt_text(ui, &p, "", "Halfway. Take the IEMs out and put them back in, so the result averages over two insertions instead of one.");
        if theme::primary(ui, &p, "Continue", ui.available_width(), 40.0, semi(13.0), true).clicked() {
            app.tt_continue();
        }
    } else {
        let can = asking && !paused;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if !m {
                let w = (ui.available_width() - 10.0) / 2.0;
                if theme::button_sized(ui, &p, "← No, it blends in", vec2(w, 44.0), false, can, med(13.0)).clicked() {
                    app.tt_answer_stair(false);
                }
                if theme::button_sized(ui, &p, "Yes, it sticks out →", vec2(w, 44.0), false, can, med(13.0)).clicked() {
                    app.tt_answer_stair(true);
                }
            } else {
                let w = (ui.available_width() - 20.0) / 3.0;
                if theme::button_sized(ui, &p, "← Second is quieter", vec2(w, 44.0), false, can, med(13.0)).clicked() {
                    app.tt_answer_pair(Loudness::Quieter);
                }
                if theme::button_sized(ui, &p, "Same · space", vec2(w, 44.0), false, can, med(13.0)).clicked() {
                    app.tt_answer_pair(Loudness::Equal);
                }
                if theme::button_sized(ui, &p, "Second is louder →", vec2(w, 44.0), false, can, med(13.0)).clicked() {
                    app.tt_answer_pair(Loudness::Louder);
                }
            }
        });
    }
    ui.horizontal(|ui| {
        if button(ui, &p, "Stop", Kind::Normal, true).clicked() {
            app.tt_stop();
        }
        if button(ui, &p, "Undo · Backspace", Kind::Normal, can_undo).on_hover_text("Undo the last answer (Backspace)").clicked() {
            app.tt_undo();
        }
        right_row(ui, |ui| {
            let pl = if paused { "Resume".to_string() } else if m { "Pause · P".into() } else { "Pause · space".into() };
            if button(ui, &p, &pl, Kind::Normal, !reseat).on_hover_text("Pause").clicked() {
                app.tt_pause();
            }
            if m {
                if button(ui, &p, "Second · V", Kind::Normal, !reseat).on_hover_text("Play the second burst again (V)").clicked() {
                    app.tt_replay(Some(false));
                }
                if button(ui, &p, "First · R", Kind::Normal, !reseat).on_hover_text("Play the first burst again (R)").clicked() {
                    app.tt_replay(Some(true));
                }
            } else if button(ui, &p, "Replay · R", Kind::Normal, !reseat).on_hover_text("Play it again (R)").clicked() {
                app.tt_replay(None);
            }
        });
    });
    ui.vertical_centered(|ui| {
        text(ui, if m { "Keys are on the buttons · enter plays both again" } else { "Keys are on the buttons" }, mono(10.5), p.faint);
    });
}

/// Same layout as the SoundEQ site: arrows answer, R / V replay, space is "same" when matching
/// and pause when listening for a band. Backspace undoes the last answer.
fn tt_keys(app: &mut App, ui: &mut Ui) {
    if ui.ctx().memory(|m| m.focused().is_some()) {
        return;
    }
    let m = app.tt.session.as_ref().map(|s| s.method == Method::Match).unwrap_or(false);
    let k = ui.input(|i| {
        if i.modifiers.ctrl || i.modifiers.alt || i.modifiers.mac_cmd {
            return None;
        }
        [Key::ArrowLeft, Key::ArrowRight, Key::Space, Key::ArrowDown, Key::R, Key::V, Key::Enter, Key::P, Key::Backspace].into_iter().find(|k| i.key_pressed(*k))
    });
    match k {
        Some(Key::ArrowLeft) => {
            if m {
                app.tt_answer_pair(Loudness::Quieter)
            } else {
                app.tt_answer_stair(false)
            }
        }
        Some(Key::ArrowRight) => {
            if m {
                app.tt_answer_pair(Loudness::Louder)
            } else {
                app.tt_answer_stair(true)
            }
        }
        Some(Key::Space) => {
            if m {
                app.tt_answer_pair(Loudness::Equal)
            } else {
                app.tt_pause()
            }
        }
        Some(Key::ArrowDown) if m => app.tt_answer_pair(Loudness::Equal),
        Some(Key::R) => app.tt_replay(if m { Some(true) } else { None }),
        Some(Key::V) if m => app.tt_replay(Some(false)),
        Some(Key::Enter) if m => app.tt_replay(None),
        Some(Key::P) => app.tt_pause(),
        Some(Key::Backspace) => app.tt_undo(),
        _ => {}
    }
}

fn tt_done(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let Some(r) = app.tt.result.clone() else {
        app.tt.view = TtView::Setup;
        return;
    };
    tt_text(ui, &p, "", &format!("Correction your ear asked for at each band, in dB (what the EQ should add). Only peaks and dips are kept; overall level and tilt are removed. It saves as {}.", r.name));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = (ui.available_width() - 36.0) / 7.0;
        for [f, c] in &r.bands {
            egui::Frame::new().fill(p.row).corner_radius(8).inner_margin(Margin::symmetric(4, 8)).show(ui, |ui| {
                ui.set_width(w - 8.0);
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 5.0;
                    text(ui, format!("{}k", fixed(f / 1000.0, 1)), mono(11.0), p.muted);
                    let col = if *c > 0.0 { p.pos } else if *c < 0.0 { p.accent } else { p.faint };
                    text(ui, format!("{}{}", if *c > 0.0 { "+" } else { "" }, fixed(*c, 1)), monos(13.0), col);
                });
            });
        }
    });
    if !app.tt.done_warn.is_empty() {
        let w = app.tt.done_warn.clone();
        theme::note(ui, &p, &w, !app.tt.done_ok, false);
    }
    let can_save = app.frs.meas_rec().is_some();
    ui.horizontal(|ui| {
        if button(ui, &p, "Discard", Kind::Normal, true).clicked() {
            app.tt.result = None;
            app.tt.view = TtView::Setup;
            app.tt.warn = app.tt_setup_warn();
        }
        right_row(ui, |ui| {
            let r2 = button(ui, &p, "Save profile", Kind::On, can_save);
            let r2 = if can_save { r2 } else { r2.on_hover_text("Pick a measurement to save profiles. You can still apply this result to the EQ.") };
            if r2.clicked() {
                app.tt_save();
            }
            if button(ui, &p, "Apply to EQ", Kind::Normal, true).on_hover_text("Turn this result into treble bands on the current EQ, using its flat bands").clicked() {
                let (msg, ok) = app.apply_treble_profile(&r.bands);
                app.tt.done_warn = msg;
                app.tt.done_ok = ok;
            }
        });
    });
}

// ---- log, clip, about ----

fn log(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    if head(app, ui, "Diagnostics", "Device I/O and write log.") {
        return;
    }
    egui::Frame::new().fill(p.row).stroke(Stroke::new(1.0, p.line)).corner_radius(9).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(300.0).min_scrolled_height(300.0).stick_to_bottom(true).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut s = app.log.join("\n");
            ui.add(egui::TextEdit::multiline(&mut s).font(mono(11.5)).text_color(p.muted).frame(egui::Frame::NONE).desired_width(f32::INFINITY).interactive(true));
        });
    });
}

fn clip(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let (eff, pf) = app.clip_info;
    let ap = app.auto_preamp_value();
    let title = if app.hi_gain { "Clipping, reduce level to write" } else { "This EQ will clip" };
    let sub = if app.hi_gain {
        format!("+6 dB mode is on, so this can't be written while it clips. Auto headroom sets preamp to {} dB to bring it under 0.", fixed(ap, 1))
    } else {
        format!("Auto headroom sets preamp to {} dB to remove it, or write anyway and lower your volume.", fixed(ap, 1))
    };
    if head(app, ui, title, &sub) {
        return;
    }
    theme::note(ui, &p, &format!("Peaks at +{} dB @ {} Hz, it will clip.", fixed(eff, 1), eq::js_num(pf)), true, true);
    right_row(ui, |ui| {
        if button(ui, &p, "Auto headroom & write", Kind::On, true).clicked() {
            app.push_undo();
            let v = app.auto_preamp_value();
            app.set_preamp(v);
            app.close_modal();
            if app.hi_gain && app.eff_peak().0 > 0.1 {
                app.write_note = Some("Still clipping after Auto, lower some band gains before writing in +6 dB mode.".into());
                return;
            }
            app.do_write();
        }
        if !app.hi_gain && button(ui, &p, "Write anyway", Kind::Normal, true).clicked() {
            app.close_modal();
            app.do_write();
        }
        if button(ui, &p, "Cancel", Kind::Normal, true).clicked() {
            app.close_modal();
        }
    });
}

fn about(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    if head(app, ui, "FreeDSP Studio", &format!("v{} · open-source EQ editor for USB-C DSP cables and dongles", crate::app::VERSION)) {
        return;
    }
    para(
        ui,
        "Read, edit and write the parametric EQ that lives on the cable or dongle. Independent project, not affiliated with or endorsed by Moondrop, FiiO or any other manufacturer. If it's useful to you, a coffee is always appreciated.",
        sans(12.5),
        p.muted,
    );
    para(ui, "Auto-fit uses the approach from AutoEq by Jaakko Pasanen (MIT). Fonts: IBM Plex (SIL Open Font License).", mono(11.0), p.faint);
    egui::CollapsingHeader::new(egui::RichText::new("Font licence (IBM Plex, SIL OFL 1.1)").font(mono(11.0)).color(p.muted)).id_salt("ofl").show(ui, |ui| {
        egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
            para(ui, include_str!("../assets/fonts/OFL.txt"), mono(10.5), p.faint);
        });
    });
    ui.horizontal(|ui| {
        if button(ui, &p, "Buy me a coffee", Kind::On, true).clicked() {
            let _ = open::that("https://buymeacoffee.com/effectiveequivalent");
        }
        if button(ui, &p, "GitHub", Kind::Normal, true).clicked() {
            let _ = open::that("https://github.com/EffectiveEquivalent/FreeDSP-Studio");
        }
    });
}

// ---- measurement & target files ----

fn frs(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    if head(app, ui, "Measurement & target", "Import your own FR files (frequency / dB text), then pick a measurement and a target. Everything is saved for next time.") {
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        caps(ui, &p, "Rig");
        let rigs = ["5128", "711", "other"];
        let sel = rigs.iter().position(|r| *r == app.frs.rig).unwrap_or(0);
        if let Some(i) = theme::segmented(ui, &p, &rigs, sel, monos(11.0), vec2(14.0, 6.0), true) {
            app.set_fr_rig(rigs[i]);
        }
        text(ui, "kept separate so they can't be mixed", sans(11.0), p.muted);
    });
    let rig = app.frs.rig.clone();
    ui.columns(2, |cols| {
        for (k, ui) in cols.iter_mut().enumerate() {
            let meas = k == 0;
            ui.spacing_mut().item_spacing.y = 8.0;
            caps(ui, &p, if meas { "Measurement" } else { "Target" });
            text(ui, if meas { "Your IEM's measured response, e.g. a DUSK or MAY curve." } else { "What you're tuning toward, e.g. JM-1, DF or Harman." }, sans(10.5), p.faint);
            let names: Vec<String> = if meas { app.frs.meas.iter() } else { app.frs.targets.iter() }.filter(|(_, r)| r.rig == rig).map(|(k, _)| k.clone()).collect();
            let mut pick = None;
            let mut del = None;
            egui::ScrollArea::vertical().max_height(150.0).id_salt(("frlist", k)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 5.0;
                if names.is_empty() {
                    text(ui, format!("None for {} yet.", rig), sans(12.0), p.faint);
                }
                for n in &names {
                    let cur = if meas { app.frs.cur_meas.as_deref() == Some(n) } else { app.frs.cur_tgt.as_deref() == Some(&format!("t:{}", n)) };
                    let (c, d) = theme::list_row(ui, &p, n, None, "", cur, true);
                    if c {
                        pick = Some(n.clone());
                    }
                    if d {
                        del = Some(n.clone());
                    }
                }
            });
            if let Some(n) = pick {
                if meas {
                    if app.frs.cur_tgt.as_deref() == Some(&format!("m:{}", n)) {
                        app.frs.cur_tgt = None;
                    }
                    app.frs.cur_meas = Some(n);
                } else {
                    app.frs.cur_tgt = Some(format!("t:{}", n));
                }
                app.persist_frs();
            }
            if let Some(n) = del {
                if meas {
                    app.frs.meas.shift_remove(&n);
                    if app.frs.cur_meas.as_deref() == Some(&n) {
                        app.frs.cur_meas = None;
                    }
                    if app.frs.cur_tgt.as_deref() == Some(&format!("m:{}", n)) {
                        app.frs.cur_tgt = None;
                    }
                } else {
                    app.frs.targets.shift_remove(&n);
                    if app.frs.cur_tgt.as_deref() == Some(&format!("t:{}", n)) {
                        app.frs.cur_tgt = None;
                    }
                }
                app.persist_frs();
            }
            if button(ui, &p, if meas { "Import measurements…" } else { "Import targets…" }, Kind::Normal, true).clicked() {
                if let Some(files) = rfd::FileDialog::new().add_filter("Frequency response", &["txt", "csv", "tab"]).pick_files() {
                    app.import_fr(meas, files);
                }
            }
        }
    });
    para(ui, &app.fr_msg, sans(12.0), p.muted);
    ui.horizontal(|ui| {
        if button(ui, &p, "Clear selection", Kind::Normal, true).clicked() {
            app.frs.cur_meas = None;
            app.frs.cur_tgt = None;
            app.persist_frs();
            app.set_graph_mode(crate::app::GraphMode::Eq);
        }
        right_row(ui, |ui| {
            if button(ui, &p, "Show FR view", Kind::On, true).clicked() {
                if app.frs.cur_meas.is_none() {
                    app.fr_msg = "Import a measurement first.".into();
                } else {
                    app.close_modal();
                    app.set_graph_mode(crate::app::GraphMode::Fr);
                }
            }
        });
    });
    let _ = (fr::TT_HI, Align2::CENTER_CENTER);
}
