//! Linux UI font selection through fontconfig's `fc-match` command.
//!
//! GPUI's Linux backend hardcodes IBM Plex Sans as `.SystemUIFont`, which is
//! not the user's fontconfig default and can change CJK fallback. Linux asks
//! fontconfig for the default sans family and a Chinese-capable sans family;
//! other platforms keep GPUI's native system UI font.
use std::sync::OnceLock;

use gpui_kit::SharedString;

pub const GPUI_SYSTEM_UI: &str = ".SystemUIFont";

#[derive(Clone, Debug)]
pub struct UiFont {
    pub family: SharedString,
    pub fallbacks: Vec<String>,
}

pub fn ui_font() -> UiFont {
    static FONT: OnceLock<UiFont> = OnceLock::new();
    FONT.get_or_init(resolve_ui_font).clone()
}

#[cfg(target_os = "linux")]
fn resolve_ui_font() -> UiFont {
    let primary = match_family(&["-f", "%{family[0]}", "sans-serif"])
        .unwrap_or_else(|| GPUI_SYSTEM_UI.to_string());
    // A representative Han character forces a font that actually covers CJK;
    // `lang=zh-cn` alone can still return a Latin-only family because the
    // user's fontconfig aliases are applied first.
    let chinese = match_family(&["-f", "%{family[0]}", "sans-serif:charset=4e2d"]);
    let fallbacks = chinese
        .filter(|family| family != &primary)
        .into_iter()
        .collect();
    UiFont {
        family: primary.into(),
        fallbacks,
    }
}

#[cfg(not(target_os = "linux"))]
fn resolve_ui_font() -> UiFont {
    UiFont {
        family: GPUI_SYSTEM_UI.into(),
        fallbacks: Vec::new(),
    }
}

#[cfg(target_os = "linux")]
fn match_family(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("fc-match")
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| parse_family(&String::from_utf8_lossy(&output.stdout)))
        .flatten()
}

fn parse_family(output: &str) -> Option<String> {
    let family = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let family = family.split(',').next()?.trim().trim_matches('"').trim();
    (!family.is_empty()).then(|| family.to_string())
}

#[cfg(test)]
mod tests;
