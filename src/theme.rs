//! Colors, read from the user's active btop theme so the two tools match.

use ratatui::style::Color;
use std::{env, fs, path::PathBuf};

pub type Rgb = (u8, u8, u8);

pub struct Theme {
    pub fg: Color,
    pub title: Color,
    pub hi: Color,
    pub inactive: Color,
    pub graph_text: Color,
    pub border: Color,
    pub master_border: Color,
    pub good: Color,
    pub bad: Color,
    pub rx: [Rgb; 3],
    pub tx: [Rgb; 3],
    /// Level meter, quiet to loud.
    pub meter: [Rgb; 3],
    pub meter_off: Color,
}

impl Theme {
    pub fn load() -> Self {
        let mut t = Theme::from_str("");
        if let Some(text) = theme_path().and_then(|p| fs::read_to_string(p).ok()) {
            t = Theme::from_str(&text);
        }
        t
    }

    /// Builds a theme from btop theme syntax; missing keys use btop's default colors.
    pub fn from_str(text: &str) -> Self {
        let get = |key: &str, default: Rgb| lookup(text, key).unwrap_or(default);
        let color = |key: &str, default: Rgb| {
            let (r, g, b) = get(key, default);
            Color::Rgb(r, g, b)
        };
        Theme {
            fg: color("main_fg", (0xcc, 0xcc, 0xcc)),
            title: color("title", (0xee, 0xee, 0xee)),
            hi: color("hi_fg", (0xb5, 0x40, 0x40)),
            inactive: color("inactive_fg", (0x40, 0x40, 0x40)),
            graph_text: color("graph_text", (0x60, 0x60, 0x60)),
            border: color("div_line", (0x30, 0x30, 0x30)),
            master_border: color("net_box", (0x70, 0x30, 0x30)),
            good: color("temp_start", (0x4f, 0x9e, 0x4f)),
            bad: color("temp_end", (0xff, 0x40, 0x40)),
            rx: [
                get("download_start", (0xa6, 0x4c, 0xf0)),
                get("download_mid", (0xc0, 0x4c, 0xc0)),
                get("download_end", (0xff, 0x4c, 0x6a)),
            ],
            tx: [
                get("upload_start", (0x36, 0xb5, 0x8f)),
                get("upload_mid", (0x6b, 0xb5, 0x60)),
                get("upload_end", (0xe0, 0xb0, 0x40)),
            ],
            meter: [
                get("temp_start", (0x4f, 0x9e, 0x4f)),
                get("temp_mid", (0xe0, 0xb0, 0x40)),
                get("temp_end", (0xff, 0x40, 0x40)),
            ],
            meter_off: color("meter_bg", (0x30, 0x30, 0x30)),
        }
    }
}

fn theme_path() -> Option<PathBuf> {
    let config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let btop = config.join("btop");
    let conf = fs::read_to_string(btop.join("btop.conf")).unwrap_or_default();
    let name = conf
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("color_theme")?
                .trim()
                .strip_prefix('=')
        })
        .map(|v| v.trim().trim_matches('"').to_string())
        .unwrap_or_else(|| "current".into());
    let candidates = [
        PathBuf::from(&name), // btop allows an absolute path here
        btop.join("themes").join(format!("{name}.theme")),
        PathBuf::from(format!("/usr/share/btop/themes/{name}.theme")),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn lookup(text: &str, key: &str) -> Option<Rgb> {
    text.lines().find_map(|line| {
        let rest = line
            .trim()
            .strip_prefix("theme[")?
            .strip_prefix(key)?
            .strip_prefix(']')?;
        parse_color(rest.trim().strip_prefix('=')?.trim().trim_matches('"'))
    })
}

/// btop colors are `#rrggbb`, `#gg` (greyscale) or decimal `r g b`.
fn parse_color(s: &str) -> Option<Rgb> {
    if let Some(hex) = s.strip_prefix('#') {
        let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        return match hex.len() {
            2 => byte(0).map(|g| (g, g, g)),
            6 => Some((byte(0)?, byte(2)?, byte(4)?)),
            _ => None,
        };
    }
    let mut p = s.split_whitespace().map(|v| v.parse::<u8>().ok());
    Some((p.next()??, p.next()??, p.next()??))
}

/// Color at `t` (0..=1) along a three-stop gradient.
pub fn gradient(stops: &[Rgb; 3], t: f64) -> Color {
    let t = t.clamp(0.0, 1.0) * 2.0;
    let (a, b, f) = if t < 1.0 {
        (stops[0], stops[1], t)
    } else {
        (stops[1], stops[2], t - 1.0)
    };
    let mix = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * f).round() as u8;
    Color::Rgb(mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_btop_color_forms() {
        assert_eq!(parse_color("#1a1b26"), Some((0x1a, 0x1b, 0x26)));
        assert_eq!(parse_color("#40"), Some((0x40, 0x40, 0x40)));
        assert_eq!(parse_color("10 20 30"), Some((10, 20, 30)));
        assert_eq!(parse_color(""), None);
        assert_eq!(parse_color("#zz"), None);
    }

    #[test]
    fn lookup_matches_exact_key_only() {
        let text = "theme[main_fg_x]=\"#000000\"\n# comment\ntheme[main_fg]=\"#ffffff\"\n";
        assert_eq!(lookup(text, "main_fg"), Some((255, 255, 255)));
    }

    #[test]
    fn gradient_hits_stops() {
        let s = [(0, 0, 0), (100, 100, 100), (200, 200, 200)];
        assert_eq!(gradient(&s, 0.0), Color::Rgb(0, 0, 0));
        assert_eq!(gradient(&s, 0.5), Color::Rgb(100, 100, 100));
        assert_eq!(gradient(&s, 1.0), Color::Rgb(200, 200, 200));
    }
}
