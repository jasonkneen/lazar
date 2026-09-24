//! Semantic colours and resolved text styles.
//!
//! Renderers consume named roles (`user_surface`, `warning`, `scrollbar_thumb`)
//! rather than palette literals. That keeps theme selection independent from
//! icon selection and gives live theme previews one stable invalidation key.

use ratatui::style::{Color, Modifier, Style};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconMode {
    Nerd,
    Unicode,
    Ascii,
}

impl IconMode {
    fn from_env() -> Self {
        if let Ok(value) = std::env::var("LAZARTUI_ICONS") {
            match value.to_ascii_lowercase().as_str() {
                "nerd" => return Self::Nerd,
                "ascii" => return Self::Ascii,
                "unicode" => return Self::Unicode,
                _ => {}
            }
        }
        if std::env::var("LAZARTUI_ASCII").as_deref() == Ok("1") {
            Self::Ascii
        } else {
            Self::Unicode
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThemeKind {
    Nerv,
    GrokNight,
    GrokDay,
    TokyoNight,
    Auto,
}

impl ThemeKind {
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::Nerv,
        Self::GrokNight,
        Self::GrokDay,
        Self::TokyoNight,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Nerv => "nerv",
            Self::GrokNight => "groknight",
            Self::GrokDay => "grokday",
            Self::TokyoNight => "tokyonight",
            Self::Auto => "auto",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Nerv => "NERV",
            Self::GrokNight => "Grok Night",
            Self::GrokDay => "Grok Day",
            Self::TokyoNight => "Tokyo Night",
            Self::Auto => "Auto",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Nerv => "Lazar orange and cyan",
            Self::GrokNight => "neutral dark with restrained accents",
            Self::GrokDay => "high-contrast light theme",
            Self::TokyoNight => "blue-black with vivid accents",
            Self::Auto => "follow the macOS appearance",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "nerv" | "default" => Some(Self::Nerv),
            "groknight" | "grok-night" => Some(Self::GrokNight),
            "grokday" | "grok-day" | "light" | "day" => Some(Self::GrokDay),
            "tokyonight" | "tokyo-night" | "tokyo" => Some(Self::TokyoNight),
            "auto" | "system" => Some(Self::Auto),
            _ => None,
        }
    }

    fn resolved(self) -> Self {
        if self == Self::Auto {
            if system_appearance_is_light() {
                Self::GrokDay
            } else {
                Self::GrokNight
            }
        } else {
            self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorLevel {
    TrueColor,
    Ansi256,
    Basic,
    None,
}

impl ColorLevel {
    fn detect() -> Self {
        if let Ok(value) = std::env::var("LAZARTUI_COLOR") {
            match value.to_ascii_lowercase().as_str() {
                "truecolor" | "24bit" => return Self::TrueColor,
                "256" | "ansi256" => return Self::Ansi256,
                "16" | "basic" | "ansi" => return Self::Basic,
                "none" | "off" => return Self::None,
                _ => {}
            }
        }
        if std::env::var_os("NO_COLOR").is_some() {
            return Self::None;
        }
        let term = std::env::var("TERM")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let color_term = std::env::var("COLORTERM")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let term_program = std::env::var("TERM_PROGRAM")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if color_term.contains("truecolor")
            || color_term.contains("24bit")
            || term.contains("direct")
            || std::env::var_os("KITTY_WINDOW_ID").is_some()
            || std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
            || matches!(
                term_program.as_str(),
                "ghostty" | "iterm.app" | "wezterm" | "vscode"
            )
        {
            Self::TrueColor
        } else if term.contains("256color") {
            Self::Ansi256
        } else {
            Self::Basic
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Palette {
    bg_base: Color,
    bg_elevated: Color,
    bg_code: Color,
    bg_selection: Color,
    bg_hover: Color,
    text: Color,
    text_muted: Color,
    /// The theme's single primary colour: picker borders, titles, H1,
    /// the composer prompt glyph and its focused border all use this.
    accent: Color,
    user: Color,
    tool: Color,
    success: Color,
    error: Color,
    warning: Color,
    link: Color,
    border: Color,
    scrollbar_track: Color,
    scrollbar_thumb: Color,
}

impl Palette {
    fn for_kind(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::Nerv => Self {
                bg_base: rgb(20, 20, 24),
                bg_elevated: rgb(48, 48, 50),
                bg_code: rgb(30, 30, 36),
                bg_selection: rgb(60, 40, 10),
                bg_hover: rgb(49, 42, 30),
                text: rgb(232, 232, 236),
                text_muted: rgb(120, 120, 130),
                accent: rgb(255, 140, 0),
                user: rgb(90, 200, 220),
                tool: rgb(230, 190, 80),
                success: rgb(120, 200, 120),
                error: rgb(230, 90, 90),
                warning: rgb(230, 190, 80),
                link: rgb(90, 200, 220),
                border: rgb(90, 90, 100),
                scrollbar_track: rgb(38, 38, 42),
                scrollbar_thumb: rgb(110, 100, 90),
            },
            ThemeKind::GrokNight => Self {
                bg_base: rgb(20, 20, 21),
                bg_elevated: rgb(39, 39, 42),
                bg_code: rgb(27, 27, 30),
                bg_selection: rgb(58, 58, 64),
                bg_hover: rgb(47, 47, 52),
                text: rgb(229, 229, 231),
                text_muted: rgb(133, 133, 142),
                accent: rgb(218, 119, 86),
                user: rgb(104, 170, 220),
                tool: rgb(213, 176, 110),
                success: rgb(126, 185, 126),
                error: rgb(224, 108, 117),
                warning: rgb(224, 174, 104),
                link: rgb(112, 178, 225),
                border: rgb(72, 72, 78),
                scrollbar_track: rgb(31, 31, 34),
                scrollbar_thumb: rgb(105, 105, 114),
            },
            ThemeKind::GrokDay => Self {
                bg_base: rgb(244, 244, 245),
                bg_elevated: rgb(226, 226, 229),
                bg_code: rgb(235, 235, 237),
                bg_selection: rgb(208, 215, 226),
                bg_hover: rgb(219, 223, 229),
                text: rgb(35, 35, 39),
                text_muted: rgb(100, 100, 109),
                accent: rgb(175, 76, 35),
                user: rgb(20, 105, 155),
                tool: rgb(145, 99, 16),
                success: rgb(48, 125, 66),
                error: rgb(180, 52, 62),
                warning: rgb(151, 102, 17),
                link: rgb(28, 101, 166),
                border: rgb(174, 174, 180),
                scrollbar_track: rgb(226, 226, 229),
                scrollbar_thumb: rgb(130, 130, 138),
            },
            ThemeKind::TokyoNight => Self {
                bg_base: rgb(36, 40, 59),
                bg_elevated: rgb(41, 46, 66),
                bg_code: rgb(26, 27, 38),
                bg_selection: rgb(54, 62, 91),
                bg_hover: rgb(46, 53, 78),
                text: rgb(192, 202, 245),
                text_muted: rgb(86, 95, 137),
                accent: rgb(122, 162, 247),
                user: rgb(122, 162, 247),
                tool: rgb(224, 175, 104),
                success: rgb(158, 206, 106),
                error: rgb(247, 118, 142),
                warning: rgb(224, 175, 104),
                link: rgb(125, 207, 255),
                border: rgb(65, 72, 104),
                scrollbar_track: rgb(31, 35, 53),
                scrollbar_thumb: rgb(86, 95, 137),
            },
            ThemeKind::Auto => unreachable!("auto is resolved before palette construction"),
        }
    }

    fn quantized(mut self, level: ColorLevel) -> Self {
        let q = |color| quantize(color, level);
        self.bg_base = q(self.bg_base);
        self.bg_elevated = q(self.bg_elevated);
        self.bg_code = q(self.bg_code);
        self.bg_selection = q(self.bg_selection);
        self.bg_hover = q(self.bg_hover);
        self.text = q(self.text);
        self.text_muted = q(self.text_muted);
        self.accent = q(self.accent);
        self.user = q(self.user);
        self.tool = q(self.tool);
        self.success = q(self.success);
        self.error = q(self.error);
        self.warning = q(self.warning);
        self.link = q(self.link);
        self.border = q(self.border);
        self.scrollbar_track = q(self.scrollbar_track);
        self.scrollbar_thumb = q(self.scrollbar_thumb);
        self
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub selected_kind: ThemeKind,
    pub resolved_kind: ThemeKind,
    pub canvas: Style,
    pub accent: Style,
    pub title: Style,
    pub dim: Style,
    pub user: Style,
    pub user_text: Style,
    pub user_surface: Style,
    pub assistant_surface: Style,
    pub user_cap: Style,
    pub tool_cmd: Style,
    /// Arguments after the program name in a tool row: quieter than the name.
    pub tool_args: Style,
    pub tool_out: Style,
    pub ok: Style,
    pub fail: Style,
    pub warn: Style,
    pub selection: Style,
    pub hover: Style,
    pub code: Style,
    pub heading: Style,
    pub heading_h2: Style,
    pub heading_h3: Style,
    pub link: Style,
    pub quote: Style,
    pub marker: Style,
    pub composer_prompt: Style,
    pub composer_text: Style,
    pub composer_surface: Style,
    pub composer_border: Style,
    pub composer_border_active: Style,
    pub header_bg: Style,
    pub footer: Style,
    pub scrollbar_track: Style,
    pub scrollbar_thumb: Style,
    pub icon_mode: IconMode,
    /// When true, avoid requesting a bold terminal face. The terminal still
    /// owns the actual font family and weight; this lets configured light or
    /// thin faces remain visible instead of being replaced by bold variants.
    pub thin: bool,
    color_level: ColorLevel,
}

impl Theme {
    pub fn nerv() -> Self {
        Self::new(ThemeKind::Nerv, IconMode::from_env())
    }
    #[cfg(test)]
    pub fn nerv_with_icons(icon_mode: IconMode) -> Self {
        Self::new(ThemeKind::Nerv, icon_mode)
    }
    pub fn from_name(name: &str, icon_mode: IconMode) -> Option<Self> {
        ThemeKind::from_name(name).map(|kind| Self::new(kind, icon_mode))
    }

    pub fn new(selected_kind: ThemeKind, icon_mode: IconMode) -> Self {
        let resolved_kind = selected_kind.resolved();
        let color_level = ColorLevel::detect();
        let p = Palette::for_kind(resolved_kind).quantized(color_level);
        Self::build(
            selected_kind,
            resolved_kind,
            icon_mode,
            color_level,
            p,
            false,
        )
    }

    fn build(
        selected_kind: ThemeKind,
        resolved_kind: ThemeKind,
        icon_mode: IconMode,
        color_level: ColorLevel,
        p: Palette,
        thin: bool,
    ) -> Self {
        let emphasis = |style: Style| {
            if thin {
                style
            } else {
                style.add_modifier(Modifier::BOLD)
            }
        };
        Self {
            selected_kind,
            resolved_kind,
            canvas: Style::default().bg(p.bg_base).fg(p.text),
            accent: Style::default().fg(p.accent),
            title: emphasis(Style::default().fg(p.accent)),
            dim: Style::default().fg(p.text_muted),
            user: emphasis(Style::default().fg(p.user)),
            user_text: Style::default().fg(p.text),
            user_surface: Style::default().bg(p.bg_elevated),
            assistant_surface: Style::default(),
            user_cap: Style::default().fg(p.bg_elevated).bg(p.bg_base),
            tool_cmd: Style::default().fg(p.tool),
            tool_args: Style::default().fg(p.text_muted),
            tool_out: Style::default().fg(p.text_muted),
            ok: Style::default().fg(p.success),
            fail: emphasis(Style::default().fg(p.error)),
            warn: emphasis(Style::default().fg(p.warning)),
            selection: Style::default().bg(p.bg_selection).fg(p.text),
            hover: Style::default().bg(p.bg_hover).fg(p.text),
            code: Style::default().fg(p.text).bg(p.bg_code),
            heading: emphasis(Style::default().fg(p.accent)),
            heading_h2: emphasis(Style::default().fg(p.user)),
            heading_h3: emphasis(Style::default().fg(p.tool)),
            link: Style::default()
                .fg(p.link)
                .add_modifier(Modifier::UNDERLINED),
            quote: Style::default()
                .fg(p.text_muted)
                .add_modifier(Modifier::ITALIC),
            marker: Style::default()
                .fg(p.text_muted)
                .add_modifier(Modifier::ITALIC),
            composer_prompt: emphasis(Style::default().fg(p.accent)),
            composer_text: Style::default().fg(p.text),
            composer_surface: Style::default().bg(p.bg_base),
            composer_border: Style::default().fg(p.border),
            // The focused composer border is the theme's primary, the same
            // colour as picker borders, titles and the prompt glyph. One
            // accent per theme; nothing else gets to be "the" highlight.
            composer_border_active: Style::default().fg(p.accent),
            header_bg: Style::default().bg(p.bg_code).fg(p.text),
            footer: Style::default().fg(p.text_muted),
            scrollbar_track: Style::default().fg(p.scrollbar_track),
            scrollbar_thumb: Style::default().fg(p.scrollbar_thumb),
            icon_mode,
            thin,
            color_level,
        }
    }

    pub fn name(&self) -> &'static str {
        self.selected_kind.name()
    }
    pub fn cache_key(&self) -> u64 {
        ((self.color_level as u64) << 24)
            | ((self.selected_kind as u64) << 16)
            | ((self.resolved_kind as u64) << 8)
            | self.icon_mode as u64
            | ((self.thin as u64) << 32)
    }
    pub fn with_kind(&self, kind: ThemeKind) -> Self {
        Self::new(kind, self.icon_mode).with_thin(self.thin)
    }
    pub fn with_thin(mut self, thin: bool) -> Self {
        if self.thin == thin {
            return self;
        }
        self = Self::new(self.selected_kind, self.icon_mode);
        if thin {
            for style in [
                &mut self.title,
                &mut self.user,
                &mut self.fail,
                &mut self.warn,
                &mut self.heading,
                &mut self.heading_h2,
                &mut self.heading_h3,
                &mut self.composer_prompt,
            ] {
                *style = style.remove_modifier(Modifier::BOLD);
            }
        }
        self.thin = thin;
        self
    }
    /// Markdown strong text. A thin face has no bold to lean on, so strong
    /// text takes the accent colour instead of vanishing into the body.
    pub fn strong(&self, style: Style) -> Style {
        if self.thin {
            match self.accent.fg {
                Some(fg) => style.fg(fg),
                None => style,
            }
        } else {
            style.add_modifier(Modifier::BOLD)
        }
    }
    pub fn emphasize(&self, style: Style) -> Style {
        if self.thin {
            style.remove_modifier(Modifier::BOLD)
        } else {
            style.add_modifier(Modifier::BOLD)
        }
    }
    pub fn refresh_auto(&self) -> Self {
        if self.selected_kind == ThemeKind::Auto {
            Self::new(ThemeKind::Auto, self.icon_mode).with_thin(self.thin)
        } else {
            self.clone()
        }
    }
    pub fn quantize_color(&self, color: Color) -> Color {
        quantize(color, self.color_level)
    }
    pub fn glyph(&self, unicode: &'static str, ascii: &'static str) -> &'static str {
        match self.icon_mode {
            IconMode::Nerd | IconMode::Unicode => unicode,
            IconMode::Ascii => ascii,
        }
    }
    pub fn icon(
        &self,
        nerd: &'static str,
        unicode: &'static str,
        ascii: &'static str,
    ) -> &'static str {
        match self.icon_mode {
            IconMode::Nerd => nerd,
            IconMode::Unicode => unicode,
            IconMode::Ascii => ascii,
        }
    }
    pub fn has_unicode_borders(&self) -> bool {
        self.icon_mode != IconMode::Ascii
    }
}

fn system_appearance_is_light() -> bool {
    if let Ok(value) = std::env::var("LAZARTUI_APPEARANCE") {
        return value.eq_ignore_ascii_case("light");
    }
    #[cfg(target_os = "macos")]
    {
        return Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .map(|output| {
                !String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .eq_ignore_ascii_case("dark")
            })
            .unwrap_or(false);
    }
    #[allow(unreachable_code)]
    false
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

fn quantize(color: Color, level: ColorLevel) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    match level {
        ColorLevel::TrueColor => color,
        ColorLevel::Ansi256 => Color::Indexed(rgb_to_ansi256(r, g, b)),
        ColorLevel::Basic => nearest_basic(r, g, b),
        ColorLevel::None => Color::Reset,
    }
}

fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    let cube = |value: u8| ((value as u16 * 5 + 127) / 255) as u8;
    let (ri, gi, bi) = (cube(r), cube(g), cube(b));
    16 + 36 * ri + 6 * gi + bi
}

fn nearest_basic(r: u8, g: u8, b: u8) -> Color {
    const COLORS: [(Color, (u8, u8, u8)); 16] = [
        (Color::Black, (0, 0, 0)),
        (Color::Red, (205, 49, 49)),
        (Color::Green, (13, 188, 121)),
        (Color::Yellow, (229, 229, 16)),
        (Color::Blue, (36, 114, 200)),
        (Color::Magenta, (188, 63, 188)),
        (Color::Cyan, (17, 168, 205)),
        (Color::Gray, (229, 229, 229)),
        (Color::DarkGray, (102, 102, 102)),
        (Color::LightRed, (241, 76, 76)),
        (Color::LightGreen, (35, 209, 139)),
        (Color::LightYellow, (245, 245, 67)),
        (Color::LightBlue, (59, 142, 234)),
        (Color::LightMagenta, (214, 112, 214)),
        (Color::LightCyan, (41, 184, 219)),
        (Color::White, (255, 255, 255)),
    ];
    COLORS
        .iter()
        .min_by_key(|(_, (cr, cg, cb))| {
            let dr = i32::from(r) - i32::from(*cr);
            let dg = i32::from(g) - i32::from(*cg);
            let db = i32::from(b) - i32::from(*cb);
            dr * dr + dg * dg + db * db
        })
        .map(|(color, _)| *color)
        .unwrap_or(Color::Reset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_modes_have_explicit_fallbacks() {
        assert_eq!(
            Theme::nerv_with_icons(IconMode::Nerd).icon("nf", "uni", "ascii"),
            "nf"
        );
        assert_eq!(
            Theme::nerv_with_icons(IconMode::Unicode).icon("nf", "uni", "ascii"),
            "uni"
        );
        assert_eq!(
            Theme::nerv_with_icons(IconMode::Ascii).icon("nf", "uni", "ascii"),
            "ascii"
        );
    }

    #[test]
    fn theme_names_and_aliases_are_stable() {
        for kind in ThemeKind::ALL {
            assert_eq!(ThemeKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(
            ThemeKind::from_name("grok-night"),
            Some(ThemeKind::GrokNight)
        );
        assert_eq!(ThemeKind::from_name("system"), Some(ThemeKind::Auto));
    }

    #[test]
    fn theme_identity_changes_the_render_cache_key() {
        let nerv = Theme::new(ThemeKind::Nerv, IconMode::Unicode);
        let grok = Theme::new(ThemeKind::GrokNight, IconMode::Unicode);
        assert_ne!(nerv.cache_key(), grok.cache_key());
        assert_ne!(
            nerv.cache_key(),
            Theme::new(ThemeKind::Nerv, IconMode::Ascii).cache_key()
        );
        assert_ne!(nerv.cache_key(), nerv.clone().with_thin(true).cache_key());
    }

    #[test]
    fn thin_weight_removes_forced_bold_without_dimming_text() {
        let regular = Theme::new(ThemeKind::Nerv, IconMode::Unicode);
        let thin = regular.clone().with_thin(true);
        assert!(regular.title.add_modifier.contains(Modifier::BOLD));
        assert!(!thin.title.add_modifier.contains(Modifier::BOLD));
        assert!(!thin.heading.add_modifier.contains(Modifier::BOLD));
        assert!(!thin.composer_prompt.add_modifier.contains(Modifier::BOLD));
        assert!(!thin.title.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn every_theme_has_exactly_one_primary_colour() {
        // Regression: Tokyo Night shipped with an orange `accent` (its
        // number-literal colour) for picker borders/titles/prompt glyph but a
        // blue focused composer border. Two "primaries" in one theme reads as
        // a theming bug. Every highlight surface must resolve to `accent`.
        for kind in ThemeKind::ALL {
            let resolved = kind.resolved();
            let level = ColorLevel::TrueColor;
            let palette = Palette::for_kind(resolved).quantized(level);
            let theme = Theme::build(kind, resolved, IconMode::Unicode, level, palette, false);
            let primary = theme.accent.fg.expect("accent has a colour");
            assert_eq!(theme.composer_border_active.fg, Some(primary), "{kind:?}: focused composer border");
            assert_eq!(theme.composer_prompt.fg, Some(primary), "{kind:?}: composer prompt glyph");
            assert_eq!(theme.title.fg, Some(primary), "{kind:?}: overlay titles");
            assert_eq!(theme.heading.fg, Some(primary), "{kind:?}: H1");
            assert_ne!(Some(primary), theme.dim.fg, "{kind:?}: primary must not be the muted text colour");
        }
        // Tokyo Night's primary is its blue (#7aa2f7), not its orange.
        let level = ColorLevel::TrueColor;
        let palette = Palette::for_kind(ThemeKind::TokyoNight).quantized(level);
        let tokyo = Theme::build(
            ThemeKind::TokyoNight,
            ThemeKind::TokyoNight,
            IconMode::Unicode,
            level,
            palette,
            false,
        );
        assert_eq!(
            tokyo.accent.fg,
            Some(quantize(rgb(122, 162, 247), tokyo.color_level))
        );
    }

    #[test]
    fn composer_uses_the_canvas_background() {
        for kind in ThemeKind::ALL {
            let theme = Theme::new(kind, IconMode::Unicode);
            assert_eq!(theme.composer_surface.bg, theme.canvas.bg);
        }
    }

    #[test]
    fn limited_color_quantization_is_deterministic() {
        assert_eq!(rgb_to_ansi256(255, 0, 0), 196);
        assert_eq!(nearest_basic(250, 20, 20), Color::Red);
        assert_eq!(quantize(rgb(1, 2, 3), ColorLevel::None), Color::Reset);
    }
}
