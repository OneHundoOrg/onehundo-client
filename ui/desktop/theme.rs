use eframe::egui::{
    self, Align, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame,
    Margin, RichText, Stroke, Vec2,
};

pub const PAPER: Color32 = Color32::from_rgb(0x0D, 0x0D, 0x0E);
pub const SURFACE: Color32 = Color32::from_rgb(0x17, 0x18, 0x1A);
pub const SURFACE_2: Color32 = Color32::from_rgb(0x20, 0x21, 0x24);
pub const LINE: Color32 = Color32::from_rgb(0x2C, 0x2E, 0x31);
pub const LINE_STRONG: Color32 = Color32::from_rgb(0x6E, 0x6F, 0x72);
pub const INK: Color32 = Color32::from_rgb(0xED, 0xEB, 0xE6);
pub const INK_2: Color32 = Color32::from_rgb(0xA9, 0xA7, 0xA1);
pub const INK_3: Color32 = Color32::from_rgb(0x8A, 0x88, 0x83);
pub const GILT: Color32 = Color32::from_rgb(0xE3, 0xC4, 0x6A);
pub const GILT_HOVER: Color32 = Color32::from_rgb(0xED, 0xD2, 0x82);
pub const ERROR: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x5E);
pub const WARNING: Color32 = Color32::from_rgb(0xFF, 0x8A, 0x3D);
pub const TEAL: Color32 = Color32::from_rgb(0x3F, 0xA9, 0xA4);

pub const APP_ICON_PNG: &[u8] = include_bytes!("assets/onehundo_256.png");
pub const TRAY_ICON_PNG: &[u8] = include_bytes!("assets/onehundo_64.png");
const ARCHIVO_BOLD_TTF: &[u8] = include_bytes!("assets/fonts/Archivo-Bold.ttf");
const INTER_REGULAR_TTF: &[u8] = include_bytes!("assets/fonts/Inter-Regular.ttf");
const INTER_SEMIBOLD_TTF: &[u8] = include_bytes!("assets/fonts/Inter-SemiBold.ttf");

pub fn display_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("display".into()))
}

pub fn semibold_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn apply_grid_theme(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "archivo_bold".into(),
        FontData::from_static(ARCHIVO_BOLD_TTF).into(),
    );
    fonts.font_data.insert(
        "inter_regular".into(),
        FontData::from_static(INTER_REGULAR_TTF).into(),
    );
    fonts.font_data.insert(
        "inter_semibold".into(),
        FontData::from_static(INTER_SEMIBOLD_TTF).into(),
    );

    if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
        prop.insert(0, "inter_regular".into());
    }
    fonts
        .families
        .insert(FontFamily::Name("display".into()), vec!["archivo_bold".into()]);
    fonts
        .families
        .insert(FontFamily::Name("semibold".into()), vec!["inter_semibold".into()]);
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = Vec2::new(12.0, 12.0);
        style.spacing.button_padding = Vec2::new(16.0, 9.0);
        style.spacing.interact_size = Vec2::new(40.0, 34.0);
        style.spacing.window_margin = Margin::same(24);
        style.spacing.indent = 16.0;

        let mut visuals = egui::Visuals::dark();
        visuals.dark_mode = true;
        visuals.panel_fill = PAPER;
        visuals.window_fill = SURFACE;
        visuals.extreme_bg_color = PAPER;
        visuals.faint_bg_color = SURFACE;
        visuals.code_bg_color = SURFACE_2;
        visuals.override_text_color = Some(INK);
        visuals.hyperlink_color = GILT;
        visuals.warn_fg_color = WARNING;
        visuals.error_fg_color = ERROR;
        visuals.window_corner_radius = CornerRadius::ZERO;
        visuals.menu_corner_radius = CornerRadius::ZERO;
        visuals.window_stroke = Stroke::new(1.0, LINE);
        visuals.selection.bg_fill = GILT.gamma_multiply(0.28);
        visuals.selection.stroke = Stroke::new(1.0, GILT);

        visuals.widgets.noninteractive.bg_fill = SURFACE;
        visuals.widgets.noninteractive.weak_bg_fill = SURFACE;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, INK_2);
        visuals.widgets.noninteractive.corner_radius = CornerRadius::ZERO;

        visuals.widgets.inactive.bg_fill = SURFACE_2;
        visuals.widgets.inactive.weak_bg_fill = SURFACE_2;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, INK);
        visuals.widgets.inactive.corner_radius = CornerRadius::ZERO;

        visuals.widgets.hovered.bg_fill = SURFACE_2;
        visuals.widgets.hovered.weak_bg_fill = SURFACE_2;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, LINE_STRONG);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, INK);
        visuals.widgets.hovered.corner_radius = CornerRadius::ZERO;

        visuals.widgets.active.bg_fill = LINE;
        visuals.widgets.active.weak_bg_fill = LINE;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0, INK);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, INK);
        visuals.widgets.active.corner_radius = CornerRadius::ZERO;

        visuals.widgets.open.bg_fill = SURFACE_2;
        visuals.widgets.open.bg_stroke = Stroke::new(1.0, LINE_STRONG);
        visuals.widgets.open.corner_radius = CornerRadius::ZERO;

        style.visuals = visuals;
    });
}

pub fn load_app_icon() -> Option<egui::IconData> {
    eframe::icon_data::from_png_bytes(APP_ICON_PNG).ok()
}

pub fn grid_card<R>(
    ui: &mut egui::Ui,
    top_accent: Option<Color32>,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let resp = Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::ZERO)
        .inner_margin(Margin::same(22))
        .show(ui, add_contents);

    if let Some(accent) = top_accent {
        let rect = resp.response.rect;
        ui.painter().line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(2.0, accent),
        );
    }
    resp
}

pub fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let btn = egui::Button::new(
        RichText::new(label)
            .font(semibold_font(13.5))
            .color(PAPER),
    )
    .fill(GILT)
    .stroke(Stroke::new(1.0, GILT))
    .corner_radius(CornerRadius::ZERO)
    .min_size(Vec2::new(0.0, 36.0));
    let resp = ui.add(btn);
    if resp.hovered() {
        ui.painter().rect_stroke(
            resp.rect,
            CornerRadius::ZERO,
            Stroke::new(1.0, GILT_HOVER),
            egui::StrokeKind::Inside,
        );
    }
    resp
}

pub fn secondary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let btn = egui::Button::new(
        RichText::new(label)
            .font(semibold_font(13.5))
            .color(INK),
    )
    .fill(SURFACE_2)
    .stroke(Stroke::new(1.0, LINE_STRONG))
    .corner_radius(CornerRadius::ZERO)
    .min_size(Vec2::new(0.0, 36.0));
    ui.add(btn)
}

pub fn subtle_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let btn = egui::Button::new(RichText::new(label).size(12.5).color(INK_2))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::ZERO)
        .min_size(Vec2::new(0.0, 30.0));
    ui.add(btn)
}

pub fn status_badge(ui: &mut egui::Ui, label: &str, dot_color: Color32) {
    Frame::new()
        .fill(SURFACE_2)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::ZERO)
        .inner_margin(Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(7.0), egui::Sense::hover());
                ui.painter().rect_filled(dot_rect, CornerRadius::ZERO, dot_color);
                ui.label(
                    RichText::new(label)
                        .font(semibold_font(12.0))
                        .color(INK),
                );
            });
        });
}

pub fn progress_bar(ui: &mut egui::Ui, unlocked: usize, total: usize, height: f32) {
    let width = ui.available_width().max(60.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, SURFACE_2);
    painter.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, LINE),
        egui::StrokeKind::Inside,
    );
    if total > 0 && unlocked > 0 {
        let ratio = (unlocked.min(total) as f32) / (total as f32);
        let fill_w = (rect.width() * ratio).max(2.0).min(rect.width());
        let fill_rect = egui::Rect::from_min_size(rect.min, Vec2::new(fill_w, rect.height()));
        let fill_color = if unlocked >= total { TEAL } else { GILT };
        painter.rect_filled(fill_rect, CornerRadius::ZERO, fill_color);
    }
}

pub fn toggle_switch(ui: &mut egui::Ui, value: &mut bool) -> egui::Response {
    let desired_size = Vec2::new(44.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    if resp.clicked() {
        *value = !*value;
        resp.mark_changed();
    }
    let painter = ui.painter();
    let track_fill = if *value { GILT } else { SURFACE_2 };
    let border_col = if resp.hovered() {
        INK
    } else if *value {
        GILT
    } else {
        LINE_STRONG
    };
    painter.rect_filled(rect, CornerRadius::ZERO, track_fill);
    painter.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, border_col),
        egui::StrokeKind::Inside,
    );
    let knob_x = if *value {
        rect.right() - 18.0
    } else {
        rect.left() + 4.0
    };
    let knob_rect = egui::Rect::from_min_size(egui::pos2(knob_x, rect.top() + 4.0), Vec2::splat(14.0));
    let knob_col = if *value { PAPER } else { INK_2 };
    painter.rect_filled(knob_rect, CornerRadius::ZERO, knob_col);
    resp
}

pub fn section_heading(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).font(display_font(18.0)).color(INK));
}

pub fn muted_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(INK_2));
}

pub fn wordmark_square(ui: &mut egui::Ui) {
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let (sq, _) = ui.allocate_exact_size(Vec2::splat(18.0), egui::Sense::hover());
        ui.painter().rect_filled(sq, CornerRadius::ZERO, INK);
        // Subtle gilt inner corner hairline matching favicon geometry
        let inner = sq.shrink(5.0);
        ui.painter().rect_filled(inner, CornerRadius::ZERO, PAPER);
        ui.label(
            RichText::new("ONEHUNDO")
                .font(display_font(16.0))
                .color(INK)
                .extra_letter_spacing(1.2),
        );
    });
}

pub fn empty_state(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.with_layout(egui::Layout::top_down(Align::LEFT), |ui| {
        ui.label(RichText::new(title).font(semibold_font(15.0)).color(INK));
        ui.add_space(4.0);
        ui.label(RichText::new(subtitle).size(13.0).color(INK_2));
    });
}
