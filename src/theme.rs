//! Dark theme, fonts and shared colors.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Visuals};

pub const BG0: Color32 = Color32::from_rgb(0x0c, 0x0e, 0x13);
pub const BG1: Color32 = Color32::from_rgb(0x12, 0x15, 0x1c);
pub const BG2: Color32 = Color32::from_rgb(0x18, 0x1c, 0x25);
pub const BG3: Color32 = Color32::from_rgb(0x21, 0x26, 0x32);
pub const BG4: Color32 = Color32::from_rgb(0x2b, 0x31, 0x40);
pub const BORDER: Color32 = Color32::from_rgb(0x26, 0x2c, 0x3a);
pub const TEXT: Color32 = Color32::from_rgb(0xdc, 0xe1, 0xeb);
pub const TEXT_STRONG: Color32 = Color32::from_rgb(0xf4, 0xf6, 0xfb);
pub const TEXT_WEAK: Color32 = Color32::from_rgb(0x84, 0x8d, 0xa1);
pub const ACCENT: Color32 = Color32::from_rgb(0x6d, 0x8d, 0xff);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x2a, 0x36, 0x66);
pub const GREEN: Color32 = Color32::from_rgb(0x3d, 0xd6, 0x9f);
pub const YELLOW: Color32 = Color32::from_rgb(0xf2, 0xbb, 0x4b);
pub const RED: Color32 = Color32::from_rgb(0xff, 0x6b, 0x7d);
pub const FOLDER: Color32 = Color32::from_rgb(0xf5, 0xc4, 0x5e);
pub const CYAN: Color32 = Color32::from_rgb(0x5c, 0xc8, 0xf0);

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        FontData::from_static(include_bytes!("../assets/fonts/Inter-Regular.otf")).into(),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        FontData::from_static(include_bytes!("../assets/fonts/Inter-SemiBold.otf")).into(),
    );
    // Inter first, egui's bundled fonts stay as fallback for icons/emoji.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".into());
    fonts.families.insert(
        FontFamily::Name("bold".into()),
        {
            let mut v = vec!["inter-semibold".to_string()];
            v.extend(fonts.families[&FontFamily::Proportional].iter().skip(1).cloned());
            v
        },
    );
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(13.5, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(18.0, FontFamily::Name("bold".into()))),
            (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
        ]
        .into();

        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(8.0, 6.0);
        s.button_padding = egui::vec2(10.0, 5.0);
        s.interact_size.y = 26.0;
        s.window_margin = Margin::same(16);
        s.menu_margin = Margin::same(6);
        s.scroll.bar_width = 8.0;
        s.scroll.floating = true;

        let mut v = Visuals::dark();
        v.dark_mode = true;
        v.override_text_color = None;
        v.panel_fill = BG1;
        v.window_fill = BG2;
        v.extreme_bg_color = BG0;
        v.faint_bg_color = Color32::from_rgb(0x15, 0x19, 0x21);
        v.code_bg_color = BG0;
        v.text_edit_bg_color = Some(Color32::from_rgb(0x0f, 0x12, 0x18));
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(8);
        v.window_shadow = Shadow {
            offset: [0, 10],
            blur: 30,
            spread: 0,
            color: Color32::from_black_alpha(140),
        };
        v.popup_shadow = Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        v.selection.bg_fill = ACCENT_DIM;
        v.selection.stroke = Stroke::new(1.0, TEXT_STRONG);
        v.hyperlink_color = ACCENT;
        v.warn_fg_color = YELLOW;
        v.error_fg_color = RED;
        v.striped = true;
        v.indent_has_left_vline = false;

        let r = CornerRadius::same(7);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = BG2;
        w.noninteractive.weak_bg_fill = BG2;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.noninteractive.corner_radius = r;

        w.inactive.bg_fill = BG3;
        w.inactive.weak_bg_fill = BG3;
        w.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x2c, 0x33, 0x44));
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.corner_radius = r;

        w.hovered.bg_fill = BG4;
        w.hovered.weak_bg_fill = BG4;
        w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3a, 0x42, 0x58));
        w.hovered.fg_stroke = Stroke::new(1.5, TEXT_STRONG);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = ACCENT_DIM;
        w.active.weak_bg_fill = ACCENT_DIM;
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.fg_stroke = Stroke::new(1.5, TEXT_STRONG);
        w.active.corner_radius = r;
        w.active.expansion = 0.0;

        w.open.bg_fill = BG3;
        w.open.weak_bg_fill = BG3;
        w.open.bg_stroke = Stroke::new(1.0, BORDER);
        w.open.fg_stroke = Stroke::new(1.0, TEXT_STRONG);
        w.open.corner_radius = r;

        style.visuals = v;
    });
}

pub fn bold(text: impl Into<String>, size: f32) -> egui::RichText {
    egui::RichText::new(text).font(FontId::new(size, FontFamily::Name("bold".into())))
}

/// Accent ("primary") button.
pub fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(Color32::WHITE).strong())
            .fill(ACCENT)
            .corner_radius(CornerRadius::same(7)),
    )
}

pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BG2)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(0))
}

pub fn human_size(n: u64) -> String {
    const UNITS: [&str; 6] = ["Б", "КБ", "МБ", "ГБ", "ТБ", "ПБ"];
    if n < 1024 {
        return format!("{n} Б");
    }
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub fn human_speed(bps: f64) -> String {
    format!("{}/с", human_size(bps as u64))
}

/// Icon + color for a file name.
pub fn file_icon(name: &str, is_dir: bool, is_link: bool) -> (&'static str, Color32) {
    if is_dir {
        return (if is_link { "🔗" } else { "📁" }, FOLDER);
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" => ("🖼", Color32::from_rgb(0xc7, 0x8b, 0xff)),
        "mp4" | "mkv" | "avi" | "mov" | "webm" => ("🎞", Color32::from_rgb(0xff, 0x8a, 0x65)),
        "mp3" | "flac" | "wav" | "ogg" | "m4a" => ("🎵", Color32::from_rgb(0xff, 0x8a, 0xc0)),
        "zip" | "gz" | "tgz" | "xz" | "bz2" | "7z" | "rar" | "tar" | "zst" => ("📦", YELLOW),
        "rs" | "py" | "js" | "ts" | "c" | "h" | "cpp" | "go" | "java" | "php" | "rb" | "sh"
        | "html" | "css" | "json" | "toml" | "yaml" | "yml" | "xml" => ("📄", CYAN),
        "pdf" => ("📕", RED),
        "txt" | "md" | "log" | "ini" | "conf" | "cfg" => ("🗋", TEXT_WEAK),
        _ => (if is_link { "🔗" } else { "🗋" }, TEXT_WEAK),
    }
}
