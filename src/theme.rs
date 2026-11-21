//! Colour palettes. The dark theme is "true dark": a pure black canvas, which looks right
//! on OLED panels and next to dark-mode games and launchers.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::WindowAppearance;
use serde::{Deserialize, Serialize};

pub struct Palette {
    pub bg: u32,
    pub panel: u32,
    pub raised: u32,
    pub hover: u32,
    pub selected: u32,
    pub border: u32,
    pub border_strong: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    /// Text and glyphs drawn on top of `accent`.
    pub on_accent: u32,
    pub danger: u32,
    pub danger_hover: u32,
    pub on_danger: u32,
    pub warning: u32,
    /// Neutral emphasis: the chosen theme's outline and the marks in the theme previews.
    pub highlight: u32,
    pub toast_bg: u32,
    pub toast_text: u32,
    /// RGBA, drawn behind dialogs.
    pub backdrop: u32,
    /// Categorical colours for letter tiles.
    pub series: [u32; 6],
    /// The overview's cache groups (installed games, uninstalled games, other apps), in
    /// that fixed order. Validated for colour-vision deficiency on the panel surface.
    pub groups: [u32; 3],
}

pub const DARK: Palette = Palette {
    bg: 0x000000,
    panel: 0x0b0b0c,
    raised: 0x141416,
    hover: 0x161618,
    selected: 0x232327,
    border: 0x1f1f23,
    border_strong: 0x2e2e34,
    text: 0xededed,
    muted: 0x9b9ba3,
    accent: 0x7cc35b,
    on_accent: 0x0a1405,
    danger: 0xcc3a31,
    danger_hover: 0xb83229,
    on_danger: 0xffffff,
    warning: 0xf2b84b,
    highlight: 0xd8d8dc,
    toast_bg: 0x1c1c1f,
    toast_text: 0xededed,
    backdrop: 0x000000c0,
    series: [0x7cc35b, 0x5b9cf0, 0xe0a33a, 0xc678dd, 0x4fc1c1, 0xe06c75],
    groups: [0x3987e5, 0xd95926, 0x199e70],
};

pub const LIGHT: Palette = Palette {
    bg: 0xf4f5f7,
    panel: 0xffffff,
    raised: 0xf1f3f5,
    hover: 0xeef0f2,
    selected: 0xe3e6ea,
    border: 0xe1e4e8,
    border_strong: 0xcbd0d6,
    text: 0x14171c,
    muted: 0x59626d,
    accent: 0x3b8526,
    on_accent: 0xffffff,
    danger: 0xd5372f,
    danger_hover: 0xbb2e27,
    on_danger: 0xffffff,
    warning: 0x9a5b00,
    highlight: 0x2a2f36,
    toast_bg: 0xffffff,
    toast_text: 0x14171c,
    backdrop: 0x0f121870,
    series: [0x4c9a2a, 0x2f6fd6, 0xc98410, 0xa24fc2, 0x1f9a9a, 0xc94a55],
    groups: [0x2a78d6, 0xeb6834, 0x1baf7a],
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    /// Follow the Windows app theme.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn is_dark(self, appearance: WindowAppearance) -> bool {
        match self {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::System => {
                matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark)
            }
        }
    }
}

// One window, one theme: a process-wide flag keeps every render helper free of
// palette plumbing. It is only written from the UI thread at the start of `render`.
static DARK_ACTIVE: AtomicBool = AtomicBool::new(true);

pub fn p() -> &'static Palette {
    if DARK_ACTIVE.load(Ordering::Relaxed) { &DARK } else { &LIGHT }
}

pub fn set_dark(dark: bool) {
    DARK_ACTIVE.store(dark, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance contrast ratio between two sRGB colours.
    fn contrast(a: u32, b: u32) -> f64 {
        fn luminance(c: u32) -> f64 {
            let channel = |v: u32| {
                let s = v as f64 / 255.0;
                if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * channel(c >> 16 & 0xff) + 0.7152 * channel(c >> 8 & 0xff) + 0.0722 * channel(c & 0xff)
        }
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_meets_wcag_aa_in_both_themes() {
        for (name, p) in [("dark", &DARK), ("light", &LIGHT)] {
            for surface in [p.bg, p.panel, p.raised, p.selected] {
                assert!(contrast(p.text, surface) >= 4.5, "{name}: text on {surface:06x}");
                assert!(contrast(p.muted, surface) >= 4.5, "{name}: muted on {surface:06x}");
            }
            assert!(contrast(p.warning, p.panel) >= 4.5, "{name}: warning text");
            assert!(contrast(p.on_accent, p.accent) >= 4.5, "{name}: on accent");
            assert!(contrast(p.on_danger, p.danger) >= 4.5, "{name}: on danger");
            assert!(contrast(p.toast_text, p.toast_bg) >= 4.5, "{name}: toast");
        }
    }

    #[test]
    fn system_mode_follows_windows() {
        assert!(ThemeMode::System.is_dark(WindowAppearance::Dark));
        assert!(!ThemeMode::System.is_dark(WindowAppearance::Light));
        assert!(ThemeMode::Dark.is_dark(WindowAppearance::Light));
        assert!(!ThemeMode::Light.is_dark(WindowAppearance::Dark));
    }
}
