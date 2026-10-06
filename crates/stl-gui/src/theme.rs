//! The look of the window: colours, fonts and the few widgets egui does not have (switch, chip, card, buttons).

use eframe::egui::{self, Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};

pub const BG: Color32 = Color32::from_rgb(14, 16, 21);
pub const SIDEBAR: Color32 = Color32::from_rgb(18, 21, 28);
pub const CARD: Color32 = Color32::from_rgb(25, 29, 39);
pub const CARD_HOVER: Color32 = Color32::from_rgb(32, 37, 50);
pub const LINE: Color32 = Color32::from_rgb(40, 46, 62);
pub const TEXT: Color32 = Color32::from_rgb(232, 235, 242);
pub const MUTED: Color32 = Color32::from_rgb(141, 150, 170);
pub const FAINT: Color32 = Color32::from_rgb(93, 102, 122);
pub const ACCENT: Color32 = Color32::from_rgb(88, 140, 255);
pub const OK: Color32 = Color32::from_rgb(61, 220, 151);
pub const WARN: Color32 = Color32::from_rgb(232, 176, 74);
pub const DANGER: Color32 = Color32::from_rgb(240, 113, 107);
pub const PURPLE: Color32 = Color32::from_rgb(170, 140, 255);

pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

fn cr(r: u8) -> CornerRadius {
    CornerRadius::same(r)
}

pub fn install(ctx: &egui::Context) {
    // Segoe UI for Latin text, Microsoft YaHei behind it for Chinese mod names (egui's own fonts have no CJK glyphs)
    let mut fonts = egui::FontDefinitions::default();
    let load = |fonts: &mut egui::FontDefinitions, name: &str, path: &str| -> bool {
        match std::fs::read(path) {
            Ok(bytes) => {
                fonts.font_data.insert(name.to_string(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
                true
            }
            Err(_) => false,
        }
    };
    let regular = load(&mut fonts, "segoe", r"C:\Windows\Fonts\segoeui.ttf");
    let semibold = load(&mut fonts, "segoe-bold", r"C:\Windows\Fonts\segoeuib.ttf");
    let cjk = ["msyh.ttc", "simhei.ttf", "simsun.ttc"].iter().any(|f| load(&mut fonts, "cjk", &format!(r"C:\Windows\Fonts\{f}")));
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    if regular {
        proportional.insert(0, "segoe".into());
    }
    if cjk {
        proportional.push("cjk".into());
    }
    let mut bold_family: Vec<String> = Vec::new();
    if semibold {
        bold_family.push("segoe-bold".into());
    } else if regular {
        bold_family.push("segoe".into());
    }
    bold_family.push("Ubuntu-Light".into());
    if cjk {
        bold_family.push("cjk".into());
    }
    fonts.families.insert(bold(), bold_family);
    if cjk {
        fonts.families.entry(FontFamily::Monospace).or_default().push("cjk".into());
    }
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    use egui::TextStyle::*;
    style.text_styles = [
        (Heading, FontId::new(24.0, bold())),
        (Body, FontId::new(14.5, FontFamily::Proportional)),
        (Button, FontId::new(14.0, FontFamily::Proportional)),
        (Small, FontId::new(12.0, FontFamily::Proportional)),
        (Monospace, FontId::new(12.5, FontFamily::Monospace)),
    ]
    .into();
    style.spacing.item_spacing = Vec2::new(10.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.floating = true;

    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_corner_radius = cr(12);
    v.menu_corner_radius = cr(10);
    v.extreme_bg_color = Color32::from_rgb(11, 13, 17);
    v.faint_bg_color = CARD;
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.popup_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(120) };
    v.widgets.noninteractive.bg_fill = CARD;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.noninteractive.corner_radius = cr(8);
    for (w, fill) in [(&mut v.widgets.inactive, CARD_HOVER), (&mut v.widgets.hovered, Color32::from_rgb(40, 46, 64)), (&mut v.widgets.active, Color32::from_rgb(48, 56, 78))] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0, LINE);
        w.fg_stroke = Stroke::new(1.0, TEXT);
        w.corner_radius = cr(8);
        w.expansion = 0.0;
    }
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.6));
    v.widgets.open = v.widgets.inactive;
    ctx.set_style(style);
}

/// A rounded panel on the dark background.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new().fill(CARD).corner_radius(cr(12)).stroke(Stroke::new(1.0, LINE)).inner_margin(Margin::same(14)).show(ui, add).inner
}

/// A small coloured label ("Steam", "for v3.14.*", "made for this build").
pub fn chip(ui: &mut Ui, text: &str, color: Color32) -> Response {
    Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(cr(9))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| ui.label(RichText::new(text).size(11.5).color(color)))
        .response
}

/// An on/off switch.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let size = Vec2::new(38.0, 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(response.id, *on, 0.12);
    let track = if *on { ACCENT } else { Color32::from_rgb(52, 59, 78) };
    let track = if response.hovered() { track.gamma_multiply(1.12) } else { track };
    ui.painter().rect_filled(rect, cr(11), track);
    let x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 8.0, Color32::WHITE);
    response
}

/// The main button: filled with the accent colour.
pub fn primary_button(ui: &mut Ui, text: &str, size: Vec2, enabled: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let fill = if !enabled {
        Color32::from_rgb(38, 44, 60)
    } else if response.is_pointer_button_down_on() {
        ACCENT.gamma_multiply(0.8)
    } else if response.hovered() {
        ACCENT.gamma_multiply(1.12)
    } else {
        ACCENT
    };
    ui.painter().rect_filled(rect, cr(10), fill);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, FontId::new(17.0, bold()), if enabled { Color32::WHITE } else { FAINT });
    response
}

/// A flat button that shows its shape on hover.
pub fn ghost_button(ui: &mut Ui, text: &str) -> Response {
    ghost_button_colored(ui, text, MUTED, TEXT)
}

pub fn ghost_button_colored(ui: &mut Ui, text: &str, color: Color32, hover_color: Color32) -> Response {
    let font = FontId::new(13.5, FontFamily::Proportional);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), color);
    let size = Vec2::new(galley.size().x + 22.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, cr(8), Color32::from_rgb(34, 40, 55));
    }
    let c = if response.hovered() { hover_color } else { color };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, font, c);
    response
}

/// A square button for one glyph (↑ ↓ × +).
pub fn icon_button(ui: &mut Ui, glyph: &str, enabled: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(26.0), if enabled { Sense::click() } else { Sense::hover() });
    if enabled && response.hovered() {
        ui.painter().rect_filled(rect, cr(7), Color32::from_rgb(44, 51, 70));
    }
    let color = if !enabled {
        Color32::from_rgb(60, 67, 84)
    } else if response.hovered() {
        TEXT
    } else {
        MUTED
    };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::new(15.0, FontFamily::Proportional), color);
    response
}

/// Tabs: text with an accent underline under the chosen one. Returns the index clicked.
pub fn tabs(ui: &mut Ui, labels: &[String], current: usize) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 22.0;
        for (i, l) in labels.iter().enumerate() {
            let font = FontId::new(15.0, if i == current { bold() } else { FontFamily::Proportional });
            let galley = ui.painter().layout_no_wrap(l.clone(), font.clone(), TEXT);
            let (rect, response) = ui.allocate_exact_size(Vec2::new(galley.size().x, 30.0), Sense::click());
            let color = if i == current {
                TEXT
            } else if response.hovered() {
                Color32::from_rgb(190, 197, 214)
            } else {
                MUTED
            };
            ui.painter().text(egui::pos2(rect.left(), rect.center().y - 1.0), Align2::LEFT_CENTER, l, font, color);
            if i == current {
                ui.painter().rect_filled(egui::Rect::from_min_size(egui::pos2(rect.left(), rect.bottom() - 2.0), Vec2::new(rect.width(), 2.0)), cr(1), ACCENT);
            }
            if response.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// Small spaced capitals above a group ("PLAYSETS").
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(11.5).color(FAINT).family(bold()));
}

/// A text field with the dark rounded look.
pub fn text_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    Frame::new()
        .fill(Color32::from_rgb(11, 13, 17))
        .corner_radius(cr(8))
        .stroke(Stroke::new(1.0, LINE))
        .inner_margin(Margin::symmetric(10, 2))
        .show(ui, |ui| ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(width).frame(false).margin(Margin::symmetric(0, 5))))
        .inner
}

/// A row background that lights up when the pointer is over it; call before drawing the row's content.
pub fn row_background(ui: &Ui, rect: egui::Rect, hovered: bool, selected: bool) {
    if selected {
        ui.painter().rect_filled(rect, cr(9), ACCENT.gamma_multiply(0.14));
        ui.painter().rect_stroke(rect, cr(9), Stroke::new(1.0, ACCENT.gamma_multiply(0.5)), StrokeKind::Inside);
    } else if hovered {
        ui.painter().rect_filled(rect, cr(9), CARD_HOVER);
    }
}

/// A button that looks like a row of the sidebar: full width, text at the left.
pub fn nav_button(ui: &mut Ui, text: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, cr(8), Color32::from_rgb(30, 35, 48));
    }
    let c = if response.hovered() { TEXT } else { MUTED };
    ui.painter().text(egui::pos2(rect.left() + 10.0, rect.center().y), Align2::LEFT_CENTER, text, FontId::new(13.5, FontFamily::Proportional), c);
    response
}

/// The window icon: a four-pointed star on a rounded blue square, drawn pixel by pixel.
pub fn icon() -> egui::IconData {
    let n = 64usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = ((x as f32 + 0.5) / n as f32 * 2.0 - 1.0, (y as f32 + 0.5) / n as f32 * 2.0 - 1.0);
            // rounded square
            let q = (fx.abs() - 0.78, fy.abs() - 0.78);
            let outside = (q.0.max(0.0).powi(2) + q.1.max(0.0).powi(2)).sqrt() + q.0.max(q.1).min(0.0) - 0.2;
            if outside > 0.0 {
                continue;
            }
            let t = (fy + 1.0) / 2.0;
            let (mut r, mut g, mut b) = (egui::lerp(70.0..=40.0, t), egui::lerp(120.0..=70.0, t), egui::lerp(255.0..=190.0, t));
            // the star: |x|^(2/3) + |y|^(2/3) <= 0.62^(2/3)
            let star = fx.abs().powf(2.0 / 3.0) + fy.abs().powf(2.0 / 3.0);
            if star <= 0.62f32.powf(2.0 / 3.0) {
                (r, g, b) = (255.0, 255.0, 255.0);
            }
            let i = (y * n + x) * 4;
            rgba[i] = r as u8;
            rgba[i + 1] = g as u8;
            rgba[i + 2] = b as u8;
            rgba[i + 3] = 255;
        }
    }
    egui::IconData { rgba, width: n as u32, height: n as u32 }
}
