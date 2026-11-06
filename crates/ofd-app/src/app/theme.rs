use crate::*;

/// Re-applies the reader palette to GPUI Kit after a theme mode switch.
/// `Theme::change` loads the registered theme and would otherwise overwrite the
/// accent, so the custom colors are set in a second update.
pub(crate) fn apply_component_theme(dark: bool, ui_font: &font_policy::UiFont, cx: &mut App) {
    let p = Palette::for_mode(dark);
    Theme::change(
        if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
    Theme::update(cx, |theme| {
        theme.colors.primary = rgb(p.accent).into();
        theme.colors.primary_foreground = rgb(0xFFFFFF).into();
        theme.colors.button_primary = rgb(p.accent).into();
        theme.colors.button_primary_hover = rgb(p.primary_hover).into();
        theme.colors.button_primary_active = rgb(p.primary_active).into();
        theme.colors.button_primary_foreground = rgb(0xFFFFFF).into();
        theme.colors.border = rgb(p.border).into();
        theme.colors.foreground = rgb(p.text).into();
        theme.colors.background = rgb(p.bg).into();
        theme.font_family = ui_font.family.clone();
        theme.radius = px(8.0);
    });
}
