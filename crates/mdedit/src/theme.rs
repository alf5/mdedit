//! The app's look: a `themes.css` palette (or the system's GitHub one)
//! turned into a gpui-kit theme, plus the fonts mdedit ships with.
//!
//! Only the palette's own colours are set; gpui-kit derives the rest (hover
//! and active shades, list and tab states) from them, so every one of the 34
//! themes gets a complete, consistent chrome without a hand-written table.

use std::borrow::Cow;
use std::rc::Rc;

use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::theme::{Theme, ThemeConfig, ThemeMode};
use gpui_kit::*;
use mdedit_core::themes::{self, Palette, Rgba};

/// The UI face, the reading serif and the code face, all SIL OFL.
pub const UI_FONT: &str = "Inter";
#[allow(dead_code)] // the Manuscript style's face, from M3
pub const SERIF_FONT: &str = "Source Serif 4";
pub const MONO_FONT: &str = "JetBrains Mono";

/// Register the bundled faces with GPUI's text system. GPUI finds them by
/// family name afterwards, and so does the diagram renderer's font database
/// (see `crate::fonts_db`).
pub fn register_fonts(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = FONT_FILES.iter().map(|f| Cow::Borrowed(*f)).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("mdedit: couldn't load the bundled fonts: {e}");
    }
}

pub const FONT_FILES: &[&[u8]] = &[
    include_bytes!("../../../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../../../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/Inter-Bold.ttf"),
    include_bytes!("../../../assets/fonts/Inter-Italic.ttf"),
    include_bytes!("../../../assets/fonts/Inter-BoldItalic.ttf"),
    include_bytes!("../../../assets/fonts/SourceSerif4-Regular.ttf"),
    include_bytes!("../../../assets/fonts/SourceSerif4-Bold.ttf"),
    include_bytes!("../../../assets/fonts/SourceSerif4-It.ttf"),
    include_bytes!("../../../assets/fonts/SourceSerif4-BoldIt.ttf"),
    include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../../../assets/fonts/JetBrainsMono-Bold.ttf"),
    include_bytes!("../../../assets/fonts/JetBrainsMono-Italic.ttf"),
    include_bytes!("../../../assets/fonts/JetBrainsMono-BoldItalic.ttf"),
];

fn hex(c: Rgba) -> SharedString {
    if c[3] == 255 {
        format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]).into()
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3]).into()
    }
}

fn with_alpha(c: Rgba, a: u8) -> Rgba {
    [c[0], c[1], c[2], a]
}

/// One palette as a gpui-kit theme. The chrome sits on `bg_soft` so the
/// document, on `bg`, reads as the page.
pub fn config(name: &str, p: &Palette) -> ThemeConfig {
    let mut t = ThemeConfig::default();
    t.name = name.to_string().into();
    t.mode = if p.dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    t.font_family = Some(UI_FONT.into());
    t.font_size = Some(14.);
    t.mono_font_family = Some(MONO_FONT.into());
    t.mono_font_size = Some(13.);
    t.radius = Some(6);
    t.radius_lg = Some(10);

    let c = &mut t.colors;
    c.background = Some(hex(p.bg));
    c.foreground = Some(hex(p.text));
    c.border = Some(hex(p.border));
    c.input = Some(hex(p.border));
    c.ring = Some(hex(with_alpha(p.accent, 0x99)));
    c.caret = Some(hex(p.accent));
    c.selection = Some(hex(with_alpha(p.accent, 0x40)));
    c.link = Some(hex(p.accent));

    c.muted = Some(hex(p.bg_soft));
    c.muted_foreground = Some(hex(p.text_muted));
    c.accent = Some(hex(with_alpha(p.text, 0x14)));
    c.accent_foreground = Some(hex(p.text));
    c.primary = Some(hex(p.accent));
    c.primary_foreground = Some(hex(p.accent_text));
    c.secondary = Some(hex(with_alpha(p.text, 0x0f)));
    c.secondary_foreground = Some(hex(p.text));
    c.popover = Some(hex(p.bg_raised));
    c.popover_foreground = Some(hex(p.text));

    c.danger = Some(hex(p.danger));
    c.danger_foreground = Some(hex(p.accent_text));
    c.info = Some(hex(p.alert_note));
    c.success = Some(hex(p.alert_tip));
    c.warning = Some(hex(p.alert_warning));

    c.title_bar = Some(hex(p.bg_soft));
    c.title_bar_border = Some(hex(p.border));
    c.tab_bar = Some(hex(p.bg_soft));
    c.tab = Some(hex(p.bg_soft));
    c.tab_active = Some(hex(p.bg));
    c.tab_foreground = Some(hex(p.text_muted));
    c.tab_active_foreground = Some(hex(p.text));
    c.sidebar = Some(hex(p.bg_soft));
    c.sidebar_foreground = Some(hex(p.text));
    c.sidebar_border = Some(hex(p.border));
    c.sidebar_accent = Some(hex(with_alpha(p.text, 0x14)));
    c.sidebar_accent_foreground = Some(hex(p.text));
    c.sidebar_primary = Some(hex(p.accent));
    c.sidebar_primary_foreground = Some(hex(p.accent_text));
    c.status_bar = Some(hex(p.bg_soft));
    c.status_bar_border = Some(hex(p.border));
    c.list_active = Some(hex(with_alpha(p.accent, 0x26)));
    c.list_active_border = Some(hex(with_alpha(p.accent, 0x00)));
    c.window_border = Some(hex(p.border));

    // The kit's syntax colours for this mode, on the document's background
    // so code reads as part of the page.
    let base = if p.dark { HighlightTheme::default_dark() } else { HighlightTheme::default_light() };
    let mut hl = base.style.clone();
    hl.editor_background = Some(hsla(p.bg));
    hl.editor_foreground = Some(hsla(p.text));
    hl.editor_line_number = Some(hsla(p.text_muted));
    hl.editor_active_line_number = Some(hsla(p.text));
    t.highlight = Some(hl);
    t
}

fn hsla(c: Rgba) -> Hsla {
    gpui_kit::Rgba {
        r: c[0] as f32 / 255.,
        g: c[1] as f32 / 255.,
        b: c[2] as f32 / 255.,
        a: c[3] as f32 / 255.,
    }
    .into()
}

/// Install `theme_id` (empty: follow the system) as the app's theme.
pub fn apply(theme_id: &str, window: &mut Window, cx: &mut App) {
    let (light, dark, mode) = match themes::palette(theme_id) {
        Some(p) => {
            let mode = if p.dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            };
            let c = Rc::new(config(theme_id, &p));
            (c.clone(), c, mode)
        }
        None => (
            Rc::new(config("System Light", &themes::system_palette(false))),
            Rc::new(config("System Dark", &themes::system_palette(true))),
            ThemeMode::from(window.appearance()),
        ),
    };
    Theme::update(cx, |t| {
        t.light_theme = light;
        t.dark_theme = dark;
    });
    Theme::change(mode, Some(window), cx);
}
