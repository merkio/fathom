//! Colour tokens and egui styling for the light and dark themes.
//!
//! The palettes come from the design file. Views read colours through [`palette`] instead of
//! hard-coding them, so a theme change restyles everything at once.

use application::usecases::ThemePreference;
use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Theme};

/// Every colour the views use. Fields are named for their role, not their value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Window and editor background.
    pub bg: Color32,
    /// Sidebar, panels and cards.
    pub surface: Color32,
    /// Title bar and the quietest fills.
    pub chrome: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    /// Text on a filled accent button.
    pub on_accent: Color32,
    pub accent_soft: Color32,
    pub success: Color32,
    pub danger: Color32,
    pub danger_soft: Color32,
    pub warning: Color32,
    pub warning_soft: Color32,
}

pub const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    surface: Color32::from_rgb(0xF6, 0xF7, 0xF9),
    chrome: Color32::from_rgb(0xF1, 0xF2, 0xF4),
    border: Color32::from_rgb(0xE4, 0xE6, 0xEB),
    border_strong: Color32::from_rgb(0xCB, 0xD0, 0xD8),
    text: Color32::from_rgb(0x20, 0x24, 0x2C),
    text_muted: Color32::from_rgb(0x71, 0x77, 0x84),
    text_faint: Color32::from_rgb(0xA0, 0xA6, 0xB1),
    accent: Color32::from_rgb(0x4C, 0x65, 0xD9),
    on_accent: Color32::WHITE,
    accent_soft: Color32::from_rgb(0xEA, 0xF0, 0xFF),
    success: Color32::from_rgb(0x23, 0x80, 0x60),
    danger: Color32::from_rgb(0xB9, 0x4A, 0x48),
    danger_soft: Color32::from_rgb(0xFF, 0xF0, 0xEF),
    warning: Color32::from_rgb(0x94, 0x6C, 0x20),
    warning_soft: Color32::from_rgb(0xFF, 0xF7, 0xE6),
};

pub const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x1C, 0x1E, 0x23),
    surface: Color32::from_rgb(0x23, 0x25, 0x2B),
    chrome: Color32::from_rgb(0x28, 0x2A, 0x30),
    border: Color32::from_rgb(0x34, 0x37, 0x3F),
    border_strong: Color32::from_rgb(0x55, 0x5C, 0x69),
    text: Color32::from_rgb(0xE6, 0xE8, 0xED),
    text_muted: Color32::from_rgb(0xA1, 0xA7, 0xB3),
    text_faint: Color32::from_rgb(0x7B, 0x83, 0x92),
    accent: Color32::from_rgb(0x84, 0x98, 0xFF),
    on_accent: Color32::WHITE,
    accent_soft: Color32::from_rgb(0x2A, 0x35, 0x55),
    success: Color32::from_rgb(0x6D, 0xC5, 0xA3),
    danger: Color32::from_rgb(0xE9, 0x97, 0x93),
    danger_soft: Color32::from_rgb(0x3B, 0x29, 0x2D),
    warning: Color32::from_rgb(0xE2, 0xB9, 0x67),
    warning_soft: Color32::from_rgb(0x43, 0x3A, 0x27),
};

/// The fill of primary buttons: the light accent in both themes, so white text stays readable.
pub const PRIMARY_FILL: Color32 = Color32::from_rgb(0x4C, 0x65, 0xD9);

pub fn palette_for(theme: Theme) -> &'static Palette {
    match theme {
        Theme::Light => &LIGHT,
        Theme::Dark => &DARK,
    }
}

/// The palette of the theme that is active for `ui`.
pub fn palette(ui: &egui::Ui) -> &'static Palette {
    palette_for(if ui.visuals().dark_mode {
        Theme::Dark
    } else {
        Theme::Light
    })
}

/// Installs both themes into the context and applies the user's choice.
pub fn apply(ctx: &egui::Context, preference: ThemePreference) {
    ctx.set_visuals_of(Theme::Light, visuals(Theme::Light));
    ctx.set_visuals_of(Theme::Dark, visuals(Theme::Dark));
    ctx.set_theme(match preference {
        ThemePreference::Light => egui::ThemePreference::Light,
        ThemePreference::Dark => egui::ThemePreference::Dark,
        ThemePreference::System => egui::ThemePreference::System,
    });
    let mut style = (*ctx.style_of(Theme::Light)).clone();
    tune_style(&mut style);
    ctx.set_style_of(Theme::Light, style.clone());
    let mut dark = (*ctx.style_of(Theme::Dark)).clone();
    tune_style(&mut dark);
    ctx.set_style_of(Theme::Dark, dark);
}

/// A filled accent button for the one main action of an area.
pub fn primary_button(text: impl Into<egui::WidgetText>) -> egui::Button<'static> {
    egui::Button::new(text.into().color(Color32::WHITE))
        .fill(PRIMARY_FILL)
        .stroke(Stroke::NONE)
}

/// A small upper-case muted heading ("EXPLORER", "INSTANCES").
pub fn section_label(ui: &mut egui::Ui, text: &str) {
    let p = palette(ui);
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .size(10.5)
            .color(p.text_muted)
            .strong(),
    );
}

fn tune_style(style: &mut egui::Style) {
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 26.0;
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(20.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(13.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(13.0, FontFamily::Monospace),
        ),
    ]
    .into();
}

fn visuals(theme: Theme) -> egui::Visuals {
    let p = palette_for(theme);
    let mut v = match theme {
        Theme::Light => egui::Visuals::light(),
        Theme::Dark => egui::Visuals::dark(),
    };
    let radius = CornerRadius::same(6);
    v.override_text_color = Some(p.text);
    v.panel_fill = p.bg;
    v.window_fill = p.bg;
    v.extreme_bg_color = p.bg;
    v.faint_bg_color = p.surface;
    v.code_bg_color = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.hyperlink_color = p.accent;
    v.error_fg_color = p.danger;
    v.warn_fg_color = p.warning;
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0, p.accent);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.bg;
    w.noninteractive.weak_bg_fill = p.bg;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = radius;
    for (state, fill, stroke) in [
        (&mut w.inactive, p.bg, p.border),
        (&mut w.hovered, p.surface, p.border_strong),
        (&mut w.active, p.accent_soft, p.accent),
        (&mut w.open, p.accent_soft, p.accent),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.0, p.text);
        state.corner_radius = radius;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance contrast ratio.
    fn contrast(a: Color32, b: Color32) -> f32 {
        fn lum(c: Color32) -> f32 {
            let f = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
        }
        let (la, lb) = (lum(a), lum(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn body_text_is_readable_in_both_themes() {
        for (name, p) in [("light", &LIGHT), ("dark", &DARK)] {
            assert!(contrast(p.text, p.bg) >= 7.0, "{name}: text on bg");
            assert!(
                contrast(p.text, p.surface) >= 7.0,
                "{name}: text on surface"
            );
            // The design's muted grey is ~4.4:1 on white, a hair under AA; kept as designed.
            assert!(contrast(p.text_muted, p.bg) >= 4.0, "{name}: muted on bg");
            assert!(contrast(p.accent, p.bg) >= 3.0, "{name}: accent on bg");
        }
    }

    #[test]
    fn primary_buttons_keep_white_text_readable() {
        assert!(contrast(Color32::WHITE, PRIMARY_FILL) >= 4.5);
    }

    #[test]
    fn status_colours_are_readable_on_their_soft_backgrounds() {
        for (name, p) in [("light", &LIGHT), ("dark", &DARK)] {
            assert!(contrast(p.danger, p.danger_soft) >= 4.0, "{name}: danger");
            assert!(
                contrast(p.warning, p.warning_soft) >= 4.0,
                "{name}: warning"
            );
        }
    }

    #[test]
    fn the_two_themes_differ() {
        assert_ne!(LIGHT, DARK);
        assert_ne!(palette_for(Theme::Light), palette_for(Theme::Dark));
    }
}
