//! The look of the window, in the manner of iOS: a picture behind everything, frosted-glass cards (the blurred picture seen through them), large
//! titles, inset grouped lists with hairline separators, green switches, segmented controls, capsule buttons and a tab bar at the bottom.
//! egui has none of these, so they are drawn here. Icons are drawn as strokes, so no icon font is needed.

use crate::i18n::Lang;
use eframe::egui::{
    self,
    epaint::{RectShape, Shadow},
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Id, Layout, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, TextureId, Ui, UiBuilder, Vec2,
};
use std::f32::consts::TAU;

// iOS dark system colours
pub const BLUE: Color32 = Color32::from_rgb(10, 132, 255);
pub const GREEN: Color32 = Color32::from_rgb(48, 209, 88);
pub const RED: Color32 = Color32::from_rgb(255, 69, 58);
pub const ORANGE: Color32 = Color32::from_rgb(255, 159, 10);
pub const PURPLE: Color32 = Color32::from_rgb(191, 90, 242);
pub const LABEL: Color32 = Color32::WHITE;
/// white at 60 %, 30 %, 18 % (premultiplied)
pub const SECONDARY: Color32 = Color32::from_rgba_premultiplied(141, 141, 147, 153);
pub const TERTIARY: Color32 = Color32::from_rgba_premultiplied(71, 71, 74, 77);
pub const SEGMENT_ON: Color32 = Color32::from_rgb(99, 99, 102);

/// The corner radius of every card, list, popup and picture; everything else is a capsule.
pub const RADIUS: f32 = 16.0;

pub fn white(alpha: u8) -> Color32 {
    Color32::from_rgba_premultiplied(alpha, alpha, alpha, alpha)
}

pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

fn cr(r: f32) -> CornerRadius {
    CornerRadius::same(r.round().clamp(0.0, 255.0) as u8)
}

// ------------------------------------------------------------------ fonts and style

/// Segoe UI for Latin and Cyrillic; behind it a CJK font, the one of the language first (Han characters differ between Chinese and Japanese).
pub fn install_fonts(ctx: &egui::Context, lang: Lang) {
    let mut fonts = egui::FontDefinitions::default();
    let dir = r"C:\Windows\Fonts";
    let mut load = |name: &str, file: &str| -> bool {
        match std::fs::read(format!(r"{dir}\{file}")) {
            Ok(bytes) => {
                fonts.font_data.insert(name.to_string(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
                true
            }
            Err(_) => false,
        }
    };
    let regular = load("segoe", "segoeui.ttf");
    let semibold = load("segoe-semibold", "seguisb.ttf") || load("segoe-semibold", "segoeuib.ttf");
    // (name, candidate files); the first that exists is used
    let cjk_of = |lang: Lang| -> Vec<(&'static str, &'static [&'static str])> {
        let yahei: (&str, &[&str]) = ("cjk-yahei", &["msyh.ttc", "simhei.ttf"]);
        let jheng: (&str, &[&str]) = ("cjk-jheng", &["msjh.ttc"]);
        let yugo: (&str, &[&str]) = ("cjk-yugo", &["YuGothR.ttc", "meiryo.ttc"]);
        let malgun: (&str, &[&str]) = ("cjk-malgun", &["malgun.ttf"]);
        match lang {
            Lang::ZhHans => vec![yahei],
            Lang::ZhHant => vec![jheng, yahei],
            Lang::Ja => vec![yugo, yahei],
            Lang::Ko => vec![malgun, yahei],
            _ => vec![yahei],
        }
    };
    let mut wanted = cjk_of(lang);
    for extra in [("cjk-yahei", &["msyh.ttc", "simhei.ttf"][..]), ("cjk-malgun", &["malgun.ttf"][..])] {
        if !wanted.iter().any(|w| w.0 == extra.0) {
            wanted.push(extra);
        }
    }
    let mut cjk_names = Vec::new();
    for (name, files) in wanted {
        if files.iter().any(|f| load(name, f)) {
            cjk_names.push(name.to_string());
        }
    }
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    if regular {
        proportional.insert(0, "segoe".into());
    }
    for (i, n) in cjk_names.iter().enumerate() {
        proportional.insert(if regular { 1 + i } else { i }, n.clone());
    }
    let mut bold_family: Vec<String> = Vec::new();
    if semibold {
        bold_family.push("segoe-semibold".into());
    } else if regular {
        bold_family.push("segoe".into());
    }
    bold_family.extend(cjk_names.iter().cloned());
    bold_family.push("Ubuntu-Light".into());
    fonts.families.insert(bold(), bold_family);
    fonts.families.entry(FontFamily::Monospace).or_default().extend(cjk_names);
    ctx.set_fonts(fonts);
}

pub fn install_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    use egui::TextStyle::*;
    style.text_styles = [
        (Heading, FontId::new(28.0, bold())),
        (Body, FontId::new(15.0, FontFamily::Proportional)),
        (Button, FontId::new(15.0, FontFamily::Proportional)),
        (Small, FontId::new(12.0, FontFamily::Proportional)),
        (Monospace, FontId::new(12.5, FontFamily::Monospace)),
    ]
    .into();
    style.spacing.item_spacing = vec2(10.0, 8.0);
    style.spacing.button_padding = vec2(12.0, 6.0);
    style.spacing.scroll.bar_width = 6.0;
    style.spacing.scroll.floating = true;
    style.spacing.scroll.floating_allocated_width = 0.0;
    style.interaction.selectable_labels = false;
    style.spacing.menu_margin = egui::Margin::same(6);

    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = Color32::from_rgb(44, 44, 46);
    v.window_stroke = Stroke::new(1.0, white(30));
    v.window_corner_radius = cr(RADIUS);
    v.menu_corner_radius = cr(RADIUS);
    v.extreme_bg_color = Color32::from_rgba_premultiplied(0, 0, 0, 90);
    v.faint_bg_color = white(10);
    v.hyperlink_color = BLUE;
    v.selection.bg_fill = BLUE.gamma_multiply(0.5);
    v.selection.stroke = Stroke::new(1.0, BLUE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, LABEL);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, LABEL);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, LABEL);
    v.widgets.active.fg_stroke = Stroke::new(1.0, LABEL);
    v.popup_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(120) };
    ctx.set_style(style);
}

// ------------------------------------------------------------------ the picture behind, and the glass in front of it

/// What a glass card needs to show the blurred picture "through" it: the texture, and where the window sits in it.
#[derive(Clone, Copy)]
pub struct GlassCtx {
    pub tex: Option<TextureId>,
    pub window: Rect,
    /// the part of the texture the whole window shows (cover fit)
    pub uv: Rect,
}

fn glass_id() -> Id {
    Id::new("stl-glass")
}

pub fn set_glass(ctx: &egui::Context, g: GlassCtx) {
    ctx.data_mut(|d| d.insert_temp(glass_id(), g));
}

/// The uv rectangle of an image that covers `target` (crops, keeps the aspect), with `anchor` 0..1 horizontally/vertically.
pub fn cover_uv(target: Vec2, image: Vec2, anchor: Vec2) -> Rect {
    let (ta, ia) = (target.x / target.y.max(1.0), image.x / image.y.max(1.0));
    let (w, h) = if ta > ia { (1.0, ia / ta) } else { (ta / ia, 1.0) };
    let (x, y) = ((1.0 - w) * anchor.x, (1.0 - h) * anchor.y);
    Rect::from_min_max(pos2(x, y), pos2(x + w, y + h))
}

/// An image filling a rounded rectangle, cropped to cover it.
pub fn cover_image(ui: &Ui, rect: Rect, tex: TextureId, size: Vec2, radius: f32, tint: Color32) {
    let uv = cover_uv(rect.size(), size, vec2(0.5, 0.5));
    ui.painter().add(Shape::Rect(RectShape::filled(rect, cr(radius), tint).with_texture(tex, uv)));
}

/// The frosted look of a rectangle: the blurred picture, a light veil, a hairline.
pub fn glass_shapes(ctx: &egui::Context, rect: Rect, radius: f32) -> Shape {
    let g = ctx.data(|d| d.get_temp::<GlassCtx>(glass_id()));
    let mut shapes = Vec::new();
    match g {
        Some(GlassCtx { tex: Some(tex), window, uv }) => {
            let map = |p: Pos2| {
                let t = vec2((p.x - window.left()) / window.width().max(1.0), (p.y - window.top()) / window.height().max(1.0));
                pos2(uv.left() + uv.width() * t.x, uv.top() + uv.height() * t.y)
            };
            let card_uv = Rect::from_min_max(map(rect.min), map(rect.max));
            shapes.push(Shape::Rect(RectShape::filled(rect, cr(radius), Color32::WHITE).with_texture(tex, card_uv)));
        }
        _ => shapes.push(Shape::rect_filled(rect, cr(radius), Color32::from_rgb(34, 34, 40))),
    }
    shapes.push(Shape::rect_filled(rect, cr(radius), white(15)));
    shapes.push(Shape::rect_stroke(rect, cr(radius), Stroke::new(1.0, white(26)), StrokeKind::Inside));
    Shape::Vec(shapes)
}

/// A glass card around whatever `add` puts in it.
pub fn glass<R>(ui: &mut Ui, radius: f32, margin: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    glass_with(ui, radius, margin, false, add)
}

fn glass_with<R>(ui: &mut Ui, radius: f32, margin: f32, floating: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    let slot = ui.painter().add(Shape::Noop);
    let out = egui::Frame::new().inner_margin(egui::Margin::same(margin as i8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    });
    let rect = out.response.rect;
    let shape = if floating { Shape::Vec(vec![lift(rect, radius), glass_shapes(ui.ctx(), rect, radius)]) } else { glass_shapes(ui.ctx(), rect, radius) };
    ui.painter().set(slot, shape);
    out.inner
}

/// The shadow of a floating card.
pub fn lift(rect: Rect, radius: f32) -> Shape {
    Shape::Rect(Shadow { offset: [0, 8], blur: 26, spread: 0, color: Color32::from_black_alpha(110) }.as_shape(rect, cr(radius)))
}

/// A long list of rows of one height that fills the rest of the space and scrolls: only the rows in view are built.
pub fn plain_rows(ui: &mut Ui, id: &str, row_height: f32, total: usize, add: impl FnOnce(&mut Ui, std::ops::Range<usize>)) {
    let rect = ui.available_rect_before_wrap();
    let mut child = ui.new_child(UiBuilder::new().id_salt(id).max_rect(rect).layout(Layout::top_down(Align::Min)));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    child.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).show_rows(&mut child, row_height, total, add);
    ui.advance_cursor_after_rect(rect);
}

/// A glass card over a given rectangle, with a margin, whose content is laid out by `add`.
pub fn glass_pane(ui: &mut Ui, rect: Rect, radius: f32, margin: f32, add: impl FnOnce(&mut Ui)) {
    ui.painter().add(glass_shapes(ui.ctx(), rect, radius));
    let mut child = ui.new_child(UiBuilder::new().id_salt(("glass-pane", rect.min.x as i32, rect.min.y as i32)).max_rect(rect.shrink(margin)).layout(Layout::top_down(Align::Min)));
    child.set_clip_rect(rect.shrink(1.0).intersect(ui.clip_rect()));
    add(&mut child);
}

/// Lines stacked and centred in a row of `row_h`, given that they are `content_h` tall together.
pub fn stack(ui: &mut Ui, row_h: f32, content_h: f32, add: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.add_space(((row_h - content_h) / 2.0).max(0.0));
        add(ui);
    });
}

// ------------------------------------------------------------------ lists

/// The rows of an inset grouped list: each row has a hairline above it, except the first.
pub struct Rows {
    first: bool,
    mark: bool,
}

impl Rows {
    pub fn new() -> Rows {
        Rows { first: true, mark: false }
    }

    /// The next row is the chosen one: it gets a blue wash.
    pub fn highlight_next(&mut self) {
        self.mark = true;
    }

    /// For lists that show only a window of their rows: say whether the first one shown is the list's first.
    pub fn starting_at(index: usize) -> Rows {
        Rows { first: index == 0, mark: false }
    }

    /// One row: `left` fills the row up to a trailing area `trail_w` wide, in which `right` is laid out right to left.
    pub fn row(&mut self, ui: &mut Ui, height: f32, trail_w: f32, clickable: bool, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) -> Response {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), if clickable { Sense::click() } else { Sense::hover() });
        if std::mem::take(&mut self.mark) {
            ui.painter().rect_filled(rect.shrink2(vec2(6.0, 2.0)), cr(10.0), white(34));
        } else if clickable && resp.hovered() {
            let a = if resp.is_pointer_button_down_on() { 30 } else { 16 };
            ui.painter().rect_filled(rect.shrink2(vec2(6.0, 2.0)), cr(10.0), white(a));
        }
        if !self.first {
            ui.painter().line_segment([pos2(rect.left() + 16.0, rect.top()), pos2(rect.right(), rect.top())], Stroke::new(1.0, white(22)));
        }
        self.first = false;
        // children are made with `new_child`, not `scope`: a scope would move the parent's cursor to the end of its own content, up inside the row
        let inner = rect.shrink2(vec2(16.0, 0.0));
        let gap = if trail_w > 0.0 { 10.0 } else { 0.0 };
        let left_rect = Rect::from_min_max(inner.min, pos2(inner.right() - trail_w - gap, inner.bottom()));
        let mut left_ui = ui.new_child(UiBuilder::new().id_salt((resp.id, "left")).max_rect(left_rect).layout(Layout::left_to_right(Align::Center)));
        left(&mut left_ui);
        if trail_w > 0.0 {
            let right_rect = Rect::from_min_max(pos2(inner.right() - trail_w, inner.top()), inner.max);
            let mut right_ui = ui.new_child(UiBuilder::new().id_salt((resp.id, "right")).max_rect(right_rect).layout(Layout::right_to_left(Align::Center)));
            right(&mut right_ui);
        }
        resp
    }
}

/// Small spaced capitals over a group.
pub fn section(ui: &mut Ui, text: &str) {
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.label(RichText::new(text.to_uppercase()).size(12.0).color(SECONDARY));
    });
    ui.add_space(2.0);
}

/// A line of small explanation under a group.
pub fn footnote(ui: &mut Ui, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.add_space(16.0);
        ui.label(RichText::new(text).size(12.5).color(SECONDARY));
    });
}

pub fn large_title(ui: &mut Ui, title: &str, subtitle: Option<&str>, trailing: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new(title).size(34.0).family(bold()).color(LABEL));
            if let Some(s) = subtitle {
                ui.label(RichText::new(s).size(14.0).color(SECONDARY));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), trailing);
    });
    ui.add_space(12.0);
}

// ------------------------------------------------------------------ controls

/// The iOS switch: a green capsule with a white knob.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(46.0, 28.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(response.id, *on, 0.14);
    let off = Color32::from_rgb(78, 78, 84);
    let track = Color32::from_rgb(
        egui::lerp(off.r() as f32..=GREEN.r() as f32, t) as u8,
        egui::lerp(off.g() as f32..=GREEN.g() as f32, t) as u8,
        egui::lerp(off.b() as f32..=GREEN.b() as f32, t) as u8,
    );
    ui.painter().rect_filled(rect, cr(14.0), track);
    let x = egui::lerp((rect.left() + 14.0)..=(rect.right() - 14.0), t);
    let knob = Rect::from_center_size(pos2(x, rect.center().y), Vec2::splat(24.0));
    ui.painter().add(Shadow { offset: [0, 1], blur: 4, spread: 0, color: Color32::from_black_alpha(80) }.as_shape(knob, cr(12.0)));
    ui.painter().circle_filled(knob.center(), 12.0, Color32::WHITE);
    response
}

/// A segmented control; returns the index chosen this frame.
pub fn segmented(ui: &mut Ui, labels: &[String], current: usize, width: f32) -> Option<usize> {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
    ui.painter().rect_filled(rect, cr(rect.height() / 2.0), white(30));
    let n = labels.len().max(1);
    let seg_w = (rect.width() - 4.0) / n as f32;
    let pill_x = ui.ctx().animate_value_with_time(ui.id().with("seg-pill").with(rect.min.x as i32), current as f32, 0.16);
    let pill = Rect::from_min_size(pos2(rect.left() + 2.0 + pill_x * seg_w, rect.top() + 2.0), vec2(seg_w, rect.height() - 4.0));
    ui.painter().add(Shadow { offset: [0, 1], blur: 3, spread: 0, color: Color32::from_black_alpha(70) }.as_shape(pill, cr(pill.height() / 2.0)));
    ui.painter().rect_filled(pill, cr(pill.height() / 2.0), SEGMENT_ON);
    let mut clicked = None;
    for (i, l) in labels.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + 2.0 + i as f32 * seg_w, rect.top() + 2.0), vec2(seg_w, rect.height() - 4.0));
        let resp = ui.interact(r, ui.id().with(("seg", i, rect.min.x as i32)), Sense::click());
        let font = FontId::new(13.5, if i == current { bold() } else { FontFamily::Proportional });
        ui.painter().text(r.center(), Align2::CENTER_CENTER, l, font, if i == current || resp.hovered() { LABEL } else { SECONDARY });
        if resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

#[derive(Clone, Copy, PartialEq)]
pub enum ButtonStyle {
    /// solid colour, white text
    Filled(Color32),
    /// the colour at low strength behind text in that colour
    Tinted(Color32),
    /// text only
    Plain(Color32),
}

/// A capsule button of a given size.
pub fn capsule_button(ui: &mut Ui, text: &str, size: Vec2, style: ButtonStyle, enabled: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let down = enabled && response.is_pointer_button_down_on();
    let hot = enabled && response.hovered();
    let (fill, fg) = match style {
        ButtonStyle::Filled(c) => (if !enabled { white(30) } else if down { c.gamma_multiply(0.75) } else if hot { c.gamma_multiply(1.1) } else { c }, if enabled { Color32::WHITE } else { TERTIARY }),
        ButtonStyle::Tinted(c) => (if !enabled { white(14) } else if down { c.gamma_multiply(0.4) } else if hot { c.gamma_multiply(0.3) } else { c.gamma_multiply(0.22) }, if enabled { c } else { TERTIARY }),
        ButtonStyle::Plain(c) => (if down { white(26) } else if hot { white(14) } else { Color32::TRANSPARENT }, if enabled { c } else { TERTIARY }),
    };
    ui.painter().rect_filled(rect, cr(size.y / 2.0), fill);
    let font = FontId::new(if size.y >= 44.0 { 17.0 } else { 14.5 }, bold());
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, font, fg);
    response
}

/// Fast start, gentle end: 0..1 -> 0..1.
pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// The two big controls of the Play page. The primary one is a white capsule with dark text (it reads on any picture and does not compete
/// with it); the other is plain glass. Both brighten a little under the pointer and press in when clicked.
pub fn hero_button(ui: &mut Ui, size: Vec2, label: &str, icon: Icon, primary: bool, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let hot = ui.ctx().animate_bool_with_time(resp.id.with("hot"), enabled && resp.hovered(), 0.16);
    let down = ui.ctx().animate_bool_with_time(resp.id.with("down"), enabled && resp.is_pointer_button_down_on(), 0.07);
    let r = rect.shrink(2.0 * down);
    let radius = r.height() / 2.0;
    let painter = ui.painter();
    let fg = if primary && enabled {
        painter.add(Shadow { offset: [0, 4], blur: (12.0 + 6.0 * hot) as u8, spread: 0, color: Color32::from_black_alpha((60.0 + 30.0 * hot) as u8) }.as_shape(r, cr(radius)));
        let rest = mix(Color32::from_rgb(236, 236, 240), Color32::WHITE, hot);
        painter.rect_filled(r, cr(radius), mix(rest, Color32::from_rgb(214, 214, 220), down));
        Color32::from_rgb(22, 22, 26)
    } else if enabled {
        painter.add(glass_shapes(ui.ctx(), r, radius));
        painter.rect_filled(r, cr(radius), white((8.0 + 20.0 * hot + 12.0 * down) as u8));
        LABEL
    } else {
        painter.rect_filled(r, cr(radius), white(14));
        TERTIARY
    };
    let galley = painter.layout_no_wrap(label.to_owned(), FontId::new(if primary { 18.0 } else { 16.0 }, bold()), fg);
    let icon_w = if primary { 18.0 } else { 16.0 };
    let gap = 10.0;
    let x0 = r.center().x - (icon_w + gap + galley.size().x) / 2.0;
    icon.draw(painter, pos2(x0 + icon_w / 2.0, r.center().y), icon_w, fg, 1.8);
    painter.galley(pos2(x0 + icon_w + gap, r.center().y - galley.size().y / 2.0), galley, fg);
    resp
}

/// A quiet picker: a caption over the chosen value, and a chevron; it only shows a shape under the pointer or while open.
pub fn inline_picker(ui: &mut Ui, caption: &str, text: &str, open: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
    if open || resp.hovered() {
        ui.painter().rect_filled(rect, cr(24.0), white(if open { 30 } else { 18 }));
    }
    let left = rect.left() + 18.0;
    ui.painter().text(pos2(left, rect.top() + 14.0), Align2::LEFT_CENTER, caption, FontId::new(11.5, FontFamily::Proportional), SECONDARY);
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), FontId::new(16.0, bold()), LABEL, (rect.width() - 64.0).max(20.0));
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(pos2(left, rect.top() + 23.0), galley, LABEL);
    (if open { Icon::Up } else { Icon::Down }).draw(ui.painter(), pos2(rect.right() - 22.0, rect.center().y), 14.0, SECONDARY, 1.8);
    resp
}

/// A button as wide as its text needs.
pub fn pill_button(ui: &mut Ui, text: &str, style: ButtonStyle, enabled: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(14.5, bold()), Color32::WHITE);
    capsule_button(ui, text, vec2(galley.size().x + 28.0, 32.0), style, enabled)
}

/// A capsule that is on or off (a tag in a list of tags).
pub fn toggle_chip(ui: &mut Ui, text: &str, on: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(13.0, FontFamily::Proportional), LABEL);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 22.0, 28.0), Sense::click());
    let fill = if on { white(64) } else if resp.hovered() { white(30) } else { white(16) };
    ui.painter().rect_filled(rect, cr(14.0), fill);
    if on {
        ui.painter().rect_stroke(rect, cr(14.0), Stroke::new(1.0, white(110)), StrokeKind::Inside);
    }
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, if on { LABEL } else { SECONDARY });
    resp
}

/// A thin capsule bar: `Some(fraction)` fills it, None shows a moving segment (the amount is not known yet).
pub fn progress_bar(ui: &mut Ui, fraction: Option<f32>, time: f64) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    ui.painter().rect_filled(rect, cr(3.0), white(26));
    let fill = match fraction {
        Some(f) => Rect::from_min_size(rect.min, vec2(rect.width() * f.clamp(0.0, 1.0), rect.height())),
        None => {
            let w = rect.width() * 0.3;
            let x = ((time * 0.8).fract() as f32) * (rect.width() + w) - w;
            Rect::from_min_max(pos2((rect.left() + x).max(rect.left()), rect.top()), pos2((rect.left() + x + w).min(rect.right()), rect.bottom()))
        }
    };
    if fill.width() > 0.0 {
        ui.painter().rect_filled(fill, cr(3.0), LABEL);
    }
    ui.ctx().request_repaint();
}

/// A hairline across the card, between its first row and what it holds.
pub fn divider(ui: &mut Ui) {
    ui.add_space(12.0);
    let r = ui.max_rect();
    let y = ui.cursor().top();
    ui.painter().line_segment([pos2(r.left(), y), pos2(r.right(), y)], Stroke::new(1.0, white(30)));
    ui.add_space(12.0);
}

/// A number in a small capsule (a count next to a heading).
pub fn count_badge(ui: &mut Ui, n: usize) -> Response {
    let galley = ui.painter().layout_no_wrap(n.to_string(), FontId::new(13.0, bold()), LABEL);
    let (rect, resp) = ui.allocate_exact_size(vec2((galley.size().x + 18.0).max(28.0), 24.0), Sense::hover());
    ui.painter().rect_filled(rect, cr(12.0), white(34));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, LABEL);
    resp
}

/// A small coloured capsule label.
pub fn chip(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = FontId::new(11.5, FontFamily::Proportional);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 14.0, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, cr(10.0), color.gamma_multiply(0.22));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// A round button with one glyph (add / added / remove / move).
pub fn circle_button(ui: &mut Ui, icon: Icon, fill: Color32, fg: Color32, enabled: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(30.0), if enabled { Sense::click() } else { Sense::hover() });
    let mut fill = if enabled { fill } else { white(10) };
    if enabled && response.hovered() {
        fill = fill.gamma_multiply(1.35);
    }
    if enabled && response.is_pointer_button_down_on() {
        fill = fill.gamma_multiply(0.7);
    }
    ui.painter().circle_filled(rect.center(), 13.0, fill);
    icon.draw(ui.painter(), rect.center(), 15.0, if enabled { fg } else { TERTIARY }, 1.8);
    response
}

/// A button of the window's title bar: a small round glass button; the close one turns red.
pub fn window_button(ui: &mut Ui, icon: Icon, danger: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        if danger { RED.gamma_multiply(0.7) } else { white(52) }
    } else if resp.hovered() {
        if danger { RED } else { white(40) }
    } else {
        white(18)
    };
    ui.painter().circle_filled(rect.center(), 13.0, fill);
    icon.draw(ui.painter(), rect.center(), 14.0, LABEL, 1.6);
    resp
}

/// The search field: a rounded gray capsule with a magnifier.
pub fn search_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
    ui.painter().rect_filled(rect, cr(18.0), white(30));
    Icon::Search.draw(ui.painter(), pos2(rect.left() + 20.0, rect.center().y), 16.0, SECONDARY, 1.6);
    let inner = Rect::from_min_max(pos2(rect.left() + 36.0, rect.top()), pos2(rect.right() - 12.0, rect.bottom()));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
    child.add(egui::TextEdit::singleline(text).hint_text(RichText::new(hint).color(SECONDARY)).frame(false).desired_width(inner.width()).margin(egui::Margin::symmetric(0, 8)))
}

/// A text field in a grouped list.
pub fn text_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
    ui.painter().rect_filled(rect, cr(18.0), white(30));
    let inner = rect.shrink2(vec2(16.0, 0.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
    child.add(egui::TextEdit::singleline(text).hint_text(RichText::new(hint).color(SECONDARY)).frame(false).desired_width(inner.width()).margin(egui::Margin::symmetric(0, 8)))
}

// ------------------------------------------------------------------ the tab bar

/// The tab bar: a floating glass capsule, centred at the bottom, the chosen page on a light wash. Returns the page clicked.
/// `sink` 0..1 lowers it by half its height and fades it a little (while the pointer is elsewhere).
pub fn tab_bar(ui: &mut Ui, items: &[(Icon, String)], current: usize, sink: f32) -> Option<usize> {
    let area = ui.max_rect();
    let n = items.len();
    let item_w = 92.0f32.min((area.width() - 24.0) / n as f32);
    let bar = Rect::from_center_size(pos2(area.center().x, area.bottom() - 8.0 - 29.0 + sink * 36.0), vec2(item_w * n as f32 + 12.0, 58.0));
    ui.set_opacity(1.0 - 0.35 * sink);
    ui.painter().add(Shape::Vec(vec![lift(bar, 29.0), glass_shapes(ui.ctx(), bar, 29.0)]));
    let mut clicked = None;
    // the blue wash slides from the old page to the new one
    let at = ui.ctx().animate_value_with_time(ui.id().with("tab-pill"), current as f32, 0.24);
    let pill = Rect::from_min_size(pos2(bar.left() + 6.0 + at * item_w, bar.top() + 5.0), vec2(item_w, 48.0));
    ui.painter().rect_filled(pill, cr(24.0), white(34));
    for (i, (icon, label)) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(bar.left() + 6.0 + i as f32 * item_w, bar.top() + 5.0), vec2(item_w, 48.0));
        let resp = ui.interact(r, ui.id().with(("tab", i)), Sense::click());
        let selected = i == current;
        if !selected && resp.hovered() {
            ui.painter().rect_filled(r, cr(24.0), white(14));
        }
        let color = if selected { LABEL } else if resp.hovered() { white(210) } else { SECONDARY };
        icon.draw(ui.painter(), pos2(r.center().x, r.top() + 18.0), 22.0, color, if selected { 1.9 } else { 1.6 });
        ui.painter().text(pos2(r.center().x, r.bottom() - 10.0), Align2::CENTER_CENTER, label, FontId::new(11.0, if selected { bold() } else { FontFamily::Proportional }), color);
        if resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

// ------------------------------------------------------------------ icons, drawn as strokes

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Play,
    PlayFilled,
    Resume,
    Playsets,
    Mods,
    Plugins,
    Settings,
    Search,
    Plus,
    Check,
    Close,
    Up,
    Down,
    Left,
    Right,
    Upload,
    Globe,
    Refresh,
    Minimize,
    Maximize,
    Restore,
}

impl Icon {
    /// Draws the icon in a square of side `size` around `c`.
    pub fn draw(self, p: &egui::Painter, c: Pos2, size: f32, color: Color32, width: f32) {
        let s = size;
        let st = Stroke::new(width, color);
        let at = |x: f32, y: f32| pos2(c.x + x * s, c.y + y * s);
        let line = |pts: Vec<Pos2>| {
            p.add(Shape::line(pts, st));
        };
        let closed = |pts: Vec<Pos2>| {
            p.add(Shape::closed_line(pts, st));
        };
        match self {
            Icon::Play => closed(vec![at(-0.26, -0.4), at(0.42, 0.0), at(-0.26, 0.4)]),
            Icon::PlayFilled => {
                p.add(Shape::convex_polygon(vec![at(-0.3, -0.42), at(0.46, 0.0), at(-0.3, 0.42)], color, Stroke::NONE));
            }
            Icon::Resume => {
                line(vec![at(-0.34, -0.36), at(-0.34, 0.36)]);
                closed(vec![at(-0.12, -0.36), at(0.42, 0.0), at(-0.12, 0.36)]);
            }
            Icon::Playsets => {
                for y in [-0.3, 0.0, 0.3] {
                    p.circle_filled(at(-0.4, y), width * 0.7, color);
                    line(vec![at(-0.2, y), at(0.44, y)]);
                }
            }
            Icon::Mods => {
                let v: Vec<Pos2> = (0..6).map(|k| {
                    let a = (-90.0 + 60.0 * k as f32).to_radians();
                    at(a.cos() * 0.46, a.sin() * 0.46)
                }).collect();
                closed(v.clone());
                for k in [1usize, 3, 5] {
                    line(vec![at(0.0, 0.0), v[k]]);
                }
            }
            Icon::Plugins => {
                closed(vec![at(-0.24, -0.24), at(0.24, -0.24), at(0.24, 0.24), at(-0.24, 0.24)]);
                for t in [-0.1, 0.1] {
                    line(vec![at(t, -0.24), at(t, -0.42)]);
                    line(vec![at(t, 0.24), at(t, 0.42)]);
                    line(vec![at(-0.24, t), at(-0.42, t)]);
                    line(vec![at(0.24, t), at(0.42, t)]);
                }
            }
            Icon::Settings => {
                let mut pts = Vec::new();
                for i in 0..8 {
                    let a = i as f32 * TAU / 8.0;
                    for (da, r) in [(-0.30f32, 0.34f32), (-0.2, 0.47), (0.2, 0.47), (0.30, 0.34)] {
                        let ang = a + da;
                        pts.push(at(ang.cos() * r, ang.sin() * r));
                    }
                }
                closed(pts);
                p.circle_stroke(c, 0.14 * s, st);
            }
            Icon::Search => {
                p.circle_stroke(at(-0.08, -0.08), 0.3 * s, st);
                line(vec![at(0.14, 0.14), at(0.42, 0.42)]);
            }
            Icon::Plus => {
                line(vec![at(-0.34, 0.0), at(0.34, 0.0)]);
                line(vec![at(0.0, -0.34), at(0.0, 0.34)]);
            }
            Icon::Check => line(vec![at(-0.34, 0.02), at(-0.1, 0.26), at(0.36, -0.24)]),
            Icon::Close => {
                line(vec![at(-0.28, -0.28), at(0.28, 0.28)]);
                line(vec![at(0.28, -0.28), at(-0.28, 0.28)]);
            }
            Icon::Left => line(vec![at(0.14, -0.3), at(-0.14, 0.0), at(0.14, 0.3)]),
            Icon::Right => line(vec![at(-0.14, -0.3), at(0.14, 0.0), at(-0.14, 0.3)]),
            Icon::Globe => {
                p.circle_stroke(c, 0.4 * s, st);
                line(vec![at(-0.4, 0.0), at(0.4, 0.0)]);
                let arc = |k: f32| -> Vec<Pos2> { (0..=12).map(|i| { let a = (-90.0 + 180.0 * i as f32 / 12.0).to_radians(); at(k * a.cos() * 0.4, a.sin() * 0.4) }).collect() };
                line(arc(0.45));
                line(arc(-0.45));
            }
            Icon::Upload => {
                line(vec![at(0.0, 0.2), at(0.0, -0.36)]);
                line(vec![at(-0.2, -0.16), at(0.0, -0.36), at(0.2, -0.16)]);
                line(vec![at(-0.36, 0.12), at(-0.36, 0.36), at(0.36, 0.36), at(0.36, 0.12)]);
            }
            Icon::Up => line(vec![at(-0.3, 0.14), at(0.0, -0.16), at(0.3, 0.14)]),
            Icon::Down => line(vec![at(-0.3, -0.14), at(0.0, 0.16), at(0.3, -0.14)]),
            Icon::Minimize => line(vec![at(-0.34, 0.0), at(0.34, 0.0)]),
            Icon::Maximize => closed(vec![at(-0.32, -0.32), at(0.32, -0.32), at(0.32, 0.32), at(-0.32, 0.32)]),
            Icon::Restore => {
                closed(vec![at(-0.36, -0.12), at(0.12, -0.12), at(0.12, 0.36), at(-0.36, 0.36)]);
                line(vec![at(-0.12, -0.12), at(-0.12, -0.36), at(0.36, -0.36), at(0.36, 0.12), at(0.12, 0.12)]);
            }
            Icon::Refresh => {
                let pts: Vec<Pos2> = (0..=18).map(|k| {
                    let a = (40.0 + 270.0 * k as f32 / 18.0).to_radians();
                    at(a.cos() * 0.34, a.sin() * 0.34)
                }).collect();
                let end = *pts.last().unwrap();
                line(pts);
                line(vec![pos2(end.x - 0.2 * s, end.y - 0.02 * s), end, pos2(end.x + 0.04 * s, end.y - 0.2 * s)]);
            }
        }
    }
}

/// The window icon: a four-pointed star on a rounded blue square, drawn pixel by pixel.
pub fn icon() -> egui::IconData {
    let n = 64usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = ((x as f32 + 0.5) / n as f32 * 2.0 - 1.0, (y as f32 + 0.5) / n as f32 * 2.0 - 1.0);
            let q = (fx.abs() - 0.78, fy.abs() - 0.78);
            let outside = (q.0.max(0.0).powi(2) + q.1.max(0.0).powi(2)).sqrt() + q.0.max(q.1).min(0.0) - 0.2;
            if outside > 0.0 {
                continue;
            }
            let t = (fy + 1.0) / 2.0;
            let (mut r, mut g, mut b) = (egui::lerp(70.0..=40.0, t), egui::lerp(120.0..=70.0, t), egui::lerp(255.0..=190.0, t));
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
