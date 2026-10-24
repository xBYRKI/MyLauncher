//! Minecraft-Optik: harte Kanten, Stein-Buttons mit Fase, Pixel-Schrift mit Schatten,
//! Inventar-Slots als Flächen. Alles wird gezeichnet – keine Mojang-Grafiken im Bundle.

use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Pos2,
    Rect, Response, Sense, Shape, Stroke, StrokeKind, TextStyle, TextureHandle, TextureOptions,
    Ui, Vec2, Widget,
};

/// Ein "Pixel" der Oberfläche in Punkten. Fasen, Rahmen und Icons sind Vielfache davon.
pub const PX: f32 = 2.0;

// Farbcodes aus dem Spiel (§a, §6, …)
pub const WHITE: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const GRAY: Color32 = Color32::from_rgb(0xAA, 0xAA, 0xAA);
pub const DARK_GRAY: Color32 = Color32::from_rgb(0x55, 0x55, 0x55);
pub const GREEN: Color32 = Color32::from_rgb(0x55, 0xFF, 0x55);
pub const YELLOW: Color32 = Color32::from_rgb(0xFF, 0xFF, 0x55);
pub const GOLD: Color32 = Color32::from_rgb(0xFF, 0xAA, 0x00);
pub const RED: Color32 = Color32::from_rgb(0xFF, 0x55, 0x55);
pub const AQUA: Color32 = Color32::from_rgb(0x55, 0xFF, 0xFF);

pub const TEXT: Color32 = Color32::from_rgb(0xE0, 0xE0, 0xE0);
pub const MUTED: Color32 = Color32::from_rgb(0x9A, 0x9A, 0x9A);
const OUTLINE: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);

// Inventar-Slot (dunkle Variante): oben/links Schatten, unten/rechts Licht
const SLOT_FILL: Color32 = Color32::from_rgba_premultiplied(0x10, 0x10, 0x12, 0xD8);
const SLOT_DARK: Color32 = Color32::from_rgb(0x0A, 0x0A, 0x0A);
const SLOT_LIGHT: Color32 = Color32::from_rgb(0x4C, 0x4C, 0x50);

pub fn mc_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("mc".into()))
}

pub fn body_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

// ---------- Schrift & Grundstil ----------

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "mc".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/Monocraft.ttf"))),
    );
    // Fließtext in Segoe UI, wie sonst unter Windows – die Pixel-Schrift ist für Titel,
    // Buttons und Etiketten. Segoe wird zur Laufzeit gelesen, nicht mitgeliefert.
    let segoe = std::env::var_os("WINDIR")
        .map(|w| std::path::PathBuf::from(w).join("Fonts").join("segoeui.ttf"))
        .and_then(|p| std::fs::read(p).ok());
    if let Some(bytes) = segoe {
        fonts
            .font_data
            .insert("segoe".into(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "segoe".into());
    }
    let mut mc = vec!["mc".to_string()];
    mc.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts.families.insert(FontFamily::Name("mc".into()), mc);
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, body_font(12.0)),
            (TextStyle::Body, body_font(14.5)),
            (TextStyle::Button, mc_font(13.0)),
            (TextStyle::Heading, mc_font(22.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(10.0, 6.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.scroll.bar_width = 10.0;
        style.spacing.scroll.floating = false;

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.override_text_color = Some(TEXT);
        v.panel_fill = Color32::TRANSPARENT;
        v.window_fill = Color32::from_rgb(0x1A, 0x1A, 0x1C);
        v.window_stroke = Stroke::new(PX, OUTLINE);
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.window_shadow = egui::Shadow::NONE;
        v.popup_shadow = egui::Shadow::NONE;
        v.extreme_bg_color = Color32::BLACK; // Textfelder wie im Spiel: schwarz
        v.faint_bg_color = Color32::from_rgb(0x20, 0x20, 0x22);
        v.selection.bg_fill = Color32::from_rgb(0x2E, 0x5C, 0x1E);
        v.selection.stroke = Stroke::new(1.0, GREEN);
        v.hyperlink_color = AQUA;
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = CornerRadius::ZERO;
            w.expansion = 0.0;
        }
        v.widgets.inactive.bg_fill = Color32::from_rgb(0x30, 0x30, 0x33);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x30, 0x30, 0x33);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0xA0, 0xA0, 0xA0));
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x3C, 0x3C, 0x40);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x3C, 0x3C, 0x40);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, WHITE);
        v.widgets.active.bg_stroke = Stroke::new(1.0, WHITE);
        v.widgets.open.bg_stroke = Stroke::new(1.0, WHITE);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, SLOT_LIGHT);
    });
}

// ---------- Texturen ----------

/// Deterministisches Rauschen für die Kacheln.
fn hash(x: u32, y: u32, seed: u32) -> u32 {
    let mut h = x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263) ^ seed.wrapping_mul(2_246_822_519);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^ (h >> 16)
}

/// 16×16-Kachel im Stil von Tiefenschiefer: dunkles Grau, waagrechte Schichten, Sprenkel.
pub fn deepslate(ctx: &egui::Context) -> TextureHandle {
    let mut rgba = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u32 {
        // Schichten: jede zweite bis dritte Zeile etwas heller
        let layer = [0i32, 4, -2, 6, 0, -4, 3, -1, 5, 0, -3, 4, 1, -2, 6, -1][y as usize];
        for x in 0..16u32 {
            let n = hash(x, y, 7);
            let mut v = 46 + layer + (n % 9) as i32 - 4;
            if n.is_multiple_of(23) {
                v -= 14; // dunkle Sprenkel
            } else if n.is_multiple_of(31) {
                v += 10;
            }
            let v = v.clamp(20, 90) as u8;
            rgba.extend_from_slice(&[v, v, v.saturating_add(3), 255]);
        }
    }
    ctx.load_texture(
        "deepslate",
        egui::ColorImage::from_rgba_unmultiplied([16, 16], &rgba),
        TextureOptions::NEAREST_REPEAT,
    )
}

/// Fläche mit einer Kachel füllen; `texel` = Punkte pro Kachelpixel.
pub fn tile(painter: &egui::Painter, rect: Rect, tex: &TextureHandle, texel: f32, tint: Color32) {
    let size = 16.0 * texel;
    let uv = Rect::from_min_max(
        Pos2::new(rect.min.x / size, rect.min.y / size),
        Pos2::new(rect.max.x / size, rect.max.y / size),
    );
    painter.image(tex.id(), rect, uv, tint);
}

// ---------- Grundformen ----------

/// Fase: Füllung, oben/links `light`, unten/rechts `dark`, optional schwarzer Außenrahmen.
pub fn bevel_shapes(rect: Rect, fill: Color32, light: Color32, dark: Color32, outline: bool) -> Vec<Shape> {
    let mut out = vec![];
    let mut r = rect;
    if outline {
        out.push(Shape::rect_filled(r, CornerRadius::ZERO, OUTLINE));
        r = r.shrink(PX);
    }
    out.push(Shape::rect_filled(r, CornerRadius::ZERO, fill));
    out.push(Shape::rect_filled(
        Rect::from_min_max(r.min, Pos2::new(r.max.x, r.min.y + PX)),
        CornerRadius::ZERO,
        light,
    ));
    out.push(Shape::rect_filled(
        Rect::from_min_max(r.min, Pos2::new(r.min.x + PX, r.max.y)),
        CornerRadius::ZERO,
        light,
    ));
    out.push(Shape::rect_filled(
        Rect::from_min_max(Pos2::new(r.min.x, r.max.y - PX), r.max),
        CornerRadius::ZERO,
        dark,
    ));
    out.push(Shape::rect_filled(
        Rect::from_min_max(Pos2::new(r.max.x - PX, r.min.y), r.max),
        CornerRadius::ZERO,
        dark,
    ));
    out
}

/// Text mit dem typischen harten Schatten (eine Pixelstufe versetzt, ¼ Helligkeit).
pub fn shadow_text(painter: &egui::Painter, pos: Pos2, anchor: Align2, text: &str, font: FontId, color: Color32) -> Rect {
    let shadow = Color32::from_rgb(color.r() / 4, color.g() / 4, color.b() / 4);
    let off = (font.size / 9.0).round().max(1.0);
    painter.text(pos + Vec2::splat(off), anchor, text, font.clone(), shadow);
    painter.text(pos, anchor, text, font, color)
}

/// Überschrift in Pixel-Schrift mit Schatten.
pub fn title(ui: &mut Ui, text: &str, size: f32, color: Color32) -> Response {
    let font = mc_font(size);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), color);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + Vec2::splat(2.0), Sense::hover());
    shadow_text(ui.painter(), rect.min, Align2::LEFT_TOP, text, font, color);
    resp
}

/// Abschnittstitel: gelb wie Tooltip-Überschriften im Spiel.
pub fn section(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    title(ui, text, 14.0, YELLOW);
    ui.add_space(2.0);
}

/// Fläche im Stil eines Inventar-Slots.
pub fn panel<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    panel_with(ui, Color32::TRANSPARENT, add)
}

/// Fläche mit farbigem Rand (z. B. aktive Welt).
pub fn panel_with<R>(ui: &mut Ui, accent: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let bg = ui.painter().add(Shape::Noop);
    let inner = egui::Frame::new()
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // `columns` erzwingen Blocksatz – dort linksbündig neu ansetzen
            if ui.layout().horizontal_justify() {
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), add).inner
            } else {
                add(ui)
            }
        });
    let rect = inner.response.rect;
    let mut shapes = bevel_shapes(rect, SLOT_FILL, SLOT_DARK, SLOT_LIGHT, false);
    if accent != Color32::TRANSPARENT {
        shapes.push(Shape::rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(PX * 2.0, rect.height())),
            CornerRadius::ZERO,
            accent,
        ));
    }
    ui.painter().set(bg, Shape::Vec(shapes));
    inner.inner
}

// ---------- Pixel-Icons (8×8) ----------

#[derive(Clone, Copy)]
pub enum Icon {
    Play,
    Folder,
    Gear,
    Refresh,
    Plus,
    Download,
    External,
    Home,
    Grid,
    Chest,
    Min,
    Max,
    Close,
}

impl Icon {
    fn rows(self) -> [&'static str; 8] {
        match self {
            Icon::Play => [
                ".#......", ".##.....", ".###....", ".####...", ".####...", ".###....", ".##.....", ".#......",
            ],
            Icon::Folder => [
                "........", "####....", "########", "#......#", "#......#", "#......#", "########", "........",
            ],
            Icon::Gear => [
                "..#..#..", ".######.", "###..###", ".#....#.", ".#....#.", "###..###", ".######.", "..#..#..",
            ],
            Icon::Refresh => [
                "..####.#", ".#....##", "#....###", "#.......", "#......#", "#......#", ".#....#.", "..####..",
            ],
            Icon::Plus => [
                "...##...", "...##...", "...##...", "########", "########", "...##...", "...##...", "...##...",
            ],
            Icon::Download => [
                "...##...", "...##...", "...##...", ".######.", "..####..", "...##...", "........", "########",
            ],
            Icon::External => [
                "....####", "......##", ".....#.#", "#...#..#", "#..#....", "#.......", "#.......", "######..",
            ],
            Icon::Home => [
                "...##...", "..####..", ".######.", "########", ".#....#.", ".#.##.#.", ".#.##.#.", ".######.",
            ],
            Icon::Grid => [
                "###.###.", "###.###.", "###.###.", "........", "###.###.", "###.###.", "###.###.", "........",
            ],
            Icon::Chest => [
                "########", "#......#", "#......#", "###..###", "#..##..#", "#......#", "#......#", "########",
            ],
            Icon::Min => [
                "........", "........", "........", "........", "........", "........", "########", "........",
            ],
            Icon::Max => [
                "########", "########", "#......#", "#......#", "#......#", "#......#", "#......#", "########",
            ],
            Icon::Close => [
                "##....##", ".##..##.", "..####..", "...##...", "..####..", ".##..##.", "##....##", "........",
            ],
        }
    }

    pub fn paint(self, painter: &egui::Painter, min: Pos2, scale: f32, color: Color32) {
        for (y, row) in self.rows().iter().enumerate() {
            for (x, c) in row.bytes().enumerate() {
                if c == b'#' {
                    let p = min + Vec2::new(x as f32, y as f32) * scale;
                    painter.rect_filled(Rect::from_min_size(p, Vec2::splat(scale)), CornerRadius::ZERO, color);
                }
            }
        }
    }
}

// ---------- Buttons ----------

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// Grauer Stein-Button wie im Menü.
    Stone,
    /// Grüner Spielen-Button wie im offiziellen Launcher.
    Play,
    /// Gewählter Eintrag (Navigation).
    Selected,
}

pub struct McButton<'a> {
    text: &'a str,
    icon: Option<Icon>,
    kind: Kind,
    size: f32,
    min_width: f32,
    height: f32,
    enabled: bool,
    tooltip: Option<&'a str>,
}

impl<'a> McButton<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            icon: None,
            kind: Kind::Stone,
            size: 13.0,
            min_width: 0.0,
            height: 30.0,
            enabled: true,
            tooltip: None,
        }
    }
    pub fn play(text: &'a str) -> Self {
        Self::new(text).kind(Kind::Play).icon(Icon::Play)
    }
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }
    pub fn big(mut self) -> Self {
        self.size = 18.0;
        self.height = 46.0;
        self.min_width = 200.0;
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
    pub fn tooltip(mut self, t: &'a str) -> Self {
        self.tooltip = Some(t);
        self
    }
}

impl Widget for McButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let font = mc_font(self.size);
        let galley = ui.painter().layout_no_wrap(self.text.to_string(), font.clone(), WHITE);
        let icon_w = if self.icon.is_some() { 8.0 * PX + if self.text.is_empty() { 0.0 } else { 8.0 } } else { 0.0 };
        let content_w = galley.size().x + icon_w;
        let w = (content_w + if self.text.is_empty() { 14.0 } else { 28.0 }).max(self.min_width);
        let sense = if self.enabled { Sense::click() } else { Sense::hover() };
        let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(w, self.height), sense);
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let hovered = self.enabled && resp.hovered();
        let pressed = self.enabled && resp.is_pointer_button_down_on();

        let (fill, light, dark) = match (self.kind, self.enabled) {
            (_, false) => (Color32::from_rgb(0x2C, 0x2C, 0x2C), Color32::from_rgb(0x3A, 0x3A, 0x3A), Color32::from_rgb(0x20, 0x20, 0x20)),
            (Kind::Play, _) => (Color32::from_rgb(0x3C, 0x85, 0x27), Color32::from_rgb(0x52, 0xA5, 0x35), Color32::from_rgb(0x1D, 0x4D, 0x13)),
            (Kind::Selected, _) => (Color32::from_rgb(0x3A, 0x3A, 0x3E), Color32::from_rgb(0x22, 0x22, 0x24), Color32::from_rgb(0x55, 0x55, 0x5A)),
            (Kind::Stone, _) => (Color32::from_rgb(0x6F, 0x6F, 0x6F), Color32::from_rgb(0xA8, 0xA8, 0xA8), Color32::from_rgb(0x3A, 0x3A, 0x3A)),
        };
        let (light, dark) = if pressed { (dark, light) } else { (light, dark) };
        let fill = if hovered && !pressed { lighten(fill, 14) } else { fill };
        let painter = ui.painter();
        painter.extend(bevel_shapes(rect, fill, light, dark, true));
        if hovered {
            // Hover wie in neueren Versionen: weißer Rahmen statt schwarzem
            painter.rect_stroke(rect.shrink(PX / 2.0), CornerRadius::ZERO, Stroke::new(PX, WHITE), StrokeKind::Middle);
        }

        let color = if !self.enabled {
            Color32::from_rgb(0x80, 0x80, 0x80)
        } else if hovered && self.kind == Kind::Stone {
            YELLOW
        } else {
            WHITE
        };
        let shift = if pressed { Vec2::splat(1.0) } else { Vec2::ZERO };
        let mut x = rect.center().x - content_w / 2.0;
        if let Some(icon) = self.icon {
            let shadow = Color32::from_rgb(color.r() / 4, color.g() / 4, color.b() / 4);
            let min = Pos2::new(x, rect.center().y - 4.0 * PX).round() + shift;
            icon.paint(painter, min + Vec2::splat(PX / 2.0), PX, shadow);
            icon.paint(painter, min, PX, color);
            x += icon_w;
        }
        if !self.text.is_empty() {
            let pos = Pos2::new(x, rect.center().y).round() + shift;
            shadow_text(painter, pos, Align2::LEFT_CENTER, self.text, font, color);
        }
        if let Some(t) = self.tooltip {
            resp = resp.on_hover_text(t);
        }
        resp
    }
}

/// `#ffd43f` → Farbe (Akzent aus der Launcher-Config)
pub fn parse_hex(hex: &str) -> Option<Color32> {
    let h = hex.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn lighten(c: Color32, by: u8) -> Color32 {
    Color32::from_rgb(c.r().saturating_add(by), c.g().saturating_add(by), c.b().saturating_add(by))
}

// ---------- Kleinteile ----------

/// Etikett in Pixel-Schrift: farbiger Text auf schwarzem Grund.
pub fn badge(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = mc_font(11.0);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), color);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + Vec2::new(12.0, 8.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(200));
    p.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, color.gamma_multiply(0.45)), StrokeKind::Inside);
    shadow_text(p, rect.center(), Align2::CENTER_CENTER, text, font, color);
    resp
}

/// Status-Punkt als 3×3-Pixel-Block.
pub fn dot(ui: &mut Ui, ok: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    let r = Rect::from_center_size(rect.center(), Vec2::splat(3.0 * PX));
    ui.painter().rect_filled(r.expand(1.0), CornerRadius::ZERO, OUTLINE);
    ui.painter().rect_filled(r, CornerRadius::ZERO, if ok { GREEN } else { RED });
}

/// Erfahrungsbalken als Fortschrittsanzeige.
pub fn xp_bar(ui: &mut Ui, fraction: f32, label: &str) {
    let w = ui.available_width().min(360.0);
    ui.vertical(|ui| {
        ui.set_width(w);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 5.0 * PX), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::ZERO, OUTLINE);
        let inner = rect.shrink(PX / 2.0);
        p.rect_filled(inner, CornerRadius::ZERO, Color32::from_rgb(0x1E, 0x1E, 0x1E));
        let fill = Rect::from_min_size(inner.min, Vec2::new(inner.width() * fraction.clamp(0.0, 1.0), inner.height()));
        p.rect_filled(fill, CornerRadius::ZERO, Color32::from_rgb(0x80, 0xFF, 0x20));
        p.rect_filled(
            Rect::from_min_size(fill.min, Vec2::new(fill.width(), PX)),
            CornerRadius::ZERO,
            Color32::from_rgb(0xC8, 0xFF, 0x7A),
        );
        // Segmente wie in der XP-Leiste
        let n = 18;
        for i in 1..n {
            let x = (inner.min.x + inner.width() * i as f32 / n as f32).round();
            p.rect_filled(Rect::from_min_size(Pos2::new(x, inner.min.y), Vec2::new(1.0, inner.height())), CornerRadius::ZERO, Color32::from_black_alpha(140));
        }
        ui.label(egui::RichText::new(label).font(mc_font(11.0)).color(GREEN));
    });
}

/// Kontrollkästchen: schwarzer Slot, grüner Haken aus Pixeln.
pub fn checkbox(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let resp = ui
        .horizontal(|ui| {
            let (rect, mut resp) = ui.allocate_exact_size(Vec2::splat(9.0 * PX), Sense::click());
            let p = ui.painter();
            p.extend(bevel_shapes(rect, Color32::BLACK, SLOT_DARK, SLOT_LIGHT, false));
            let border = if resp.hovered() { WHITE } else { Color32::from_rgb(0xA0, 0xA0, 0xA0) };
            p.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, border), StrokeKind::Inside);
            if *on {
                const CHECK: [(f32, f32); 6] = [(1.0, 4.0), (2.0, 5.0), (3.0, 6.0), (4.0, 5.0), (5.0, 4.0), (6.0, 3.0)];
                for (x, y) in CHECK {
                    let min = rect.min + Vec2::new(x, y - 1.0) * PX + Vec2::splat(PX / 2.0);
                    p.rect_filled(Rect::from_min_size(min, Vec2::new(PX, PX * 2.0)), CornerRadius::ZERO, GREEN);
                }
            }
            let text = ui.add(egui::Label::new(label).sense(Sense::click()));
            if resp.clicked() || text.clicked() {
                *on = !*on;
                resp.mark_changed();
            }
            resp
        })
        .inner;
    resp
}

/// Gedämpfte Zeile Fließtext.
pub fn muted(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.label(egui::RichText::new(text.into()).color(MUTED))
}
