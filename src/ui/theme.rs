use egui::{
    Color32, Context, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, Style, TextStyle,
    Visuals,
};

pub fn configure_theme(ctx: &Context) {
    let base = Style::default();

    let mut spacing = base.spacing.clone();
    spacing.item_spacing = egui::vec2(8.0, 8.0);
    spacing.window_margin = Margin::same(10);
    spacing.button_padding = egui::vec2(8.0, 5.0);
    spacing.menu_margin = Margin::same(6);
    spacing.indent = 18.0;
    spacing.scroll.bar_width = 10.0;
    spacing.combo_width = 100.0;

    // Visuals
    let mut visuals = Visuals::dark();

    // Background colors (Modern Dark Theme)
    visuals.window_fill = Color32::from_rgb(28, 28, 33);
    visuals.panel_fill = Color32::from_rgb(32, 32, 38);

    // Widgets
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(32, 32, 38);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_gray(180));

    // Inactive widgets
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(45, 45, 52);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_gray(200));

    // Hovered widgets
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(60, 60, 70);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);

    // Active widgets
    visuals.widgets.active.bg_fill = Color32::from_rgb(70, 70, 85);
    visuals.widgets.active.corner_radius = CornerRadius::same(6);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);

    // Selection
    visuals.selection.bg_fill = Color32::from_rgb(60, 100, 180);
    visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(100, 150, 255));

    // Window shadow and rounding
    visuals.window_corner_radius = CornerRadius::same(8);
    visuals.window_shadow = Shadow {
        offset: [0, 10],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(80),
    };

    // Popup shadow
    visuals.popup_shadow = Shadow {
        offset: [0, 5],
        blur: 10,
        spread: 0,
        color: Color32::from_black_alpha(60),
    };

    let style = Style {
        text_styles: [
            (
                TextStyle::Heading,
                FontId::new(20.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (
                TextStyle::Monospace,
                FontId::new(13.0, FontFamily::Monospace),
            ),
            (
                TextStyle::Button,
                FontId::new(14.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(10.0, FontFamily::Proportional),
            ),
        ]
        .into(),
        spacing,
        visuals,
        ..base
    };
    ctx.set_style(style);
}
