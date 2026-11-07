use crate::*;

/// Colors for one appearance mode. Kept separate from the component theme so
/// the reader keeps its own neutral palette while buttons and dialogs follow it.
#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub(crate) bg: u32,
    pub(crate) titlebar: u32,
    pub(crate) toolbar: u32,
    pub(crate) surface: u32,
    pub(crate) canvas: u32,
    pub(crate) sidebar: u32,
    pub(crate) border: u32,
    pub(crate) border_strong: u32,
    pub(crate) text: u32,
    pub(crate) text_secondary: u32,
    pub(crate) text_tertiary: u32,
    pub(crate) accent: u32,
    pub(crate) accent_soft: u32,
    pub(crate) primary_hover: u32,
    pub(crate) primary_active: u32,
    pub(crate) window_control_hover: u32,
    pub(crate) window_control_active: u32,
    pub(crate) window_close_hover: u32,
    pub(crate) window_close_active: u32,
    pub(crate) search_match: u32,
    pub(crate) warning: u32,
    pub(crate) error: u32,
    pub(crate) error_border: u32,
    pub(crate) error_soft: u32,
    pub(crate) dot: u32,
}

impl Palette {
    const LIGHT: Self = Self {
        bg: 0xF7F8FA,
        titlebar: 0xFBFBFC,
        toolbar: 0xFFFFFF,
        surface: 0xFFFFFF,
        canvas: 0xE9EBEF,
        sidebar: 0xF6F7F9,
        border: 0xE5E7EB,
        border_strong: 0xE3E6EB,
        text: 0x1B1E25,
        text_secondary: 0x5B6472,
        text_tertiary: 0x98A1AF,
        accent: 0x2F6FEB,
        accent_soft: 0xEDF2FE,
        primary_hover: 0x2560D4,
        primary_active: 0x1F4FB5,
        window_control_hover: 0xEDEFF2,
        window_control_active: 0xE1E4E9,
        window_close_hover: 0xE5484D,
        window_close_active: 0xC93A3F,
        search_match: 0xB45309,
        warning: 0xB45309,
        error: 0xB42318,
        error_border: 0xFCA5A5,
        error_soft: 0xFEF2F2,
        dot: 0xD5DAE2,
    };

    const DARK: Self = Self {
        bg: 0x17191E,
        titlebar: 0x17191E,
        toolbar: 0x1A1D23,
        surface: 0x1C1F26,
        canvas: 0x0F1217,
        sidebar: 0x17191E,
        border: 0x262A31,
        border_strong: 0x363C47,
        text: 0xE7EAF0,
        text_secondary: 0xA7B0BF,
        text_tertiary: 0x6F7A8B,
        accent: 0x3B76F0,
        accent_soft: 0x202A3D,
        primary_hover: 0x4B85F5,
        primary_active: 0x2F63D8,
        window_control_hover: 0x23262D,
        window_control_active: 0x2C3038,
        window_close_hover: 0xDC454B,
        window_close_active: 0xB83439,
        search_match: 0xFFD666,
        warning: 0xE0A64A,
        error: 0xFF7A7A,
        error_border: 0x7F3B3B,
        error_soft: 0x2A1B1D,
        dot: 0x3A424F,
    };

    pub(crate) fn for_mode(dark: bool) -> Self {
        if dark { Self::DARK } else { Self::LIGHT }
    }
}

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
