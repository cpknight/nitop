//! Colors from the user's btop theme, so the two tools match.
//! Missing keys follow btop: most fall back to its builtin Default palette, and
//! `meter_bg` and `graph_text` copy `inactive_fg`.

use ratatui::style::Color;
use std::{env, fs, path::Path, path::PathBuf};

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

/// btop's builtin `Default` palette (`Default_theme` in btop_theme.cpp).
const DEFAULT_FG: Rgb = (0xcc, 0xcc, 0xcc);
const DEFAULT_TITLE: Rgb = (0xee, 0xee, 0xee);
const DEFAULT_HI: Rgb = (0xb5, 0x40, 0x40);
const DEFAULT_INACTIVE: Rgb = (0x40, 0x40, 0x40);
const DEFAULT_GRAPH: Rgb = (0x60, 0x60, 0x60);
const DEFAULT_DIV: Rgb = (0x30, 0x30, 0x30);
const DEFAULT_NET_BOX: Rgb = (0x5c, 0x58, 0x8d);
const DEFAULT_TEMP: [Rgb; 3] = [(0x48, 0x97, 0xd4), (0x54, 0x74, 0xe8), (0xff, 0x40, 0xb6)];
const DEFAULT_RX: [Rgb; 3] = [(0x29, 0x1f, 0x75), (0x4f, 0x43, 0xa3), (0xb0, 0xa9, 0xde)];
const DEFAULT_TX: [Rgb; 3] = [(0x62, 0x06, 0x65), (0x7d, 0x41, 0x80), (0xdc, 0xaf, 0xde)];
const DEFAULT_METER_BG: Rgb = (0x40, 0x40, 0x40);

impl Theme {
    pub fn load() -> Self {
        let name = configured_name();
        if let Some(path) = theme_file(&name)
            && let Ok(text) = fs::read_to_string(path)
        {
            return Theme::from_str(&text);
        }
        if name == "TTY" {
            Theme::tty()
        } else {
            Theme::from_str("")
        }
    }

    /// Builds a theme from btop theme syntax.
    /// An empty string is the builtin Default palette. A partial file uses those
    /// defaults for ordinary keys, but a missing `meter_bg` or `graph_text` copies
    /// `inactive_fg`, which is what btop does when a theme file omits them.
    pub fn from_str(text: &str) -> Self {
        let partial = text.lines().any(|l| l.trim_start().starts_with("theme["));
        let pick = |key: &str, default: Rgb| lookup(text, key).unwrap_or(default);
        let inactive = pick("inactive_fg", DEFAULT_INACTIVE);
        let optional = |key: &str, default: Rgb| {
            lookup(text, key).unwrap_or(if partial { inactive } else { default })
        };
        let color = |c: Rgb| Color::Rgb(c.0, c.1, c.2);
        let temp = [
            pick("temp_start", DEFAULT_TEMP[0]),
            pick("temp_mid", DEFAULT_TEMP[1]),
            pick("temp_end", DEFAULT_TEMP[2]),
        ];
        Theme {
            fg: color(pick("main_fg", DEFAULT_FG)),
            title: color(pick("title", DEFAULT_TITLE)),
            hi: color(pick("hi_fg", DEFAULT_HI)),
            inactive: color(inactive),
            graph_text: color(optional("graph_text", DEFAULT_GRAPH)),
            border: color(pick("div_line", DEFAULT_DIV)),
            master_border: color(pick("net_box", DEFAULT_NET_BOX)),
            good: color(temp[0]),
            bad: color(temp[2]),
            rx: [
                pick("download_start", DEFAULT_RX[0]),
                pick("download_mid", DEFAULT_RX[1]),
                pick("download_end", DEFAULT_RX[2]),
            ],
            tx: [
                pick("upload_start", DEFAULT_TX[0]),
                pick("upload_mid", DEFAULT_TX[1]),
                pick("upload_end", DEFAULT_TX[2]),
            ],
            meter: temp,
            meter_off: color(optional("meter_bg", DEFAULT_METER_BG)),
        }
    }

    /// btop's builtin TTY theme is ANSI, not hex. These are xterm's default colors
    /// for those sequences. Empty mid stops in that theme are the halfway blend,
    /// because this gradient is continuous rather than btop's hard step.
    fn tty() -> Self {
        const BLUE: Rgb = (0, 0, 238);
        const MAGENTA: Rgb = (205, 0, 205);
        const BRIGHT_BLUE: Rgb = (92, 92, 255);
        const BRIGHT_CYAN: Rgb = (0, 255, 255);
        const BRIGHT_MAGENTA: Rgb = (255, 0, 255);
        const BRIGHT_BLACK: Rgb = (127, 127, 127);
        let color = |c: Rgb| Color::Rgb(c.0, c.1, c.2);
        Theme {
            fg: color((229, 229, 229)),
            title: color((255, 255, 255)),
            hi: color((255, 0, 0)),
            inactive: color(BRIGHT_BLACK),
            graph_text: color(BRIGHT_BLACK),
            border: color(BRIGHT_BLACK),
            master_border: color(MAGENTA),
            good: color(BRIGHT_BLUE),
            bad: color(BRIGHT_MAGENTA),
            rx: [BLUE, mix(BLUE, BRIGHT_BLUE), BRIGHT_BLUE],
            tx: [MAGENTA, mix(MAGENTA, BRIGHT_MAGENTA), BRIGHT_MAGENTA],
            meter: [BRIGHT_BLUE, BRIGHT_CYAN, BRIGHT_MAGENTA],
            meter_off: color(BRIGHT_BLACK),
        }
    }
}

fn mix(a: Rgb, b: Rgb) -> Rgb {
    (a.0 / 2 + b.0 / 2, a.1 / 2 + b.1 / 2, a.2 / 2 + b.2 / 2)
}

fn configured_name() -> String {
    let Some(home) = config_home() else {
        return "Default".into();
    };
    let text = fs::read_to_string(home.join("btop/btop.conf")).unwrap_or_default();
    theme_name_from(&text)
}

fn config_home() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

/// `color_theme` value. Quoted values end at the closing quote, so a trailing
/// comment is not part of the name. Absent means btop's builtin Default.
fn theme_name_from(conf: &str) -> String {
    conf.lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix("color_theme")?.trim();
            let rest = rest.strip_prefix('=')?.trim();
            Some(unquote(rest))
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Default".into())
}

fn unquote(value: &str) -> String {
    if let Some(rest) = value.strip_prefix('"') {
        let end = rest.find('"').unwrap_or(rest.len());
        return rest[..end].to_string();
    }
    value.split('#').next().unwrap_or("").trim().to_string()
}

fn theme_file(name: &str) -> Option<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = config_home() {
        dirs.push(home.join("btop/themes"));
    }
    dirs.push(PathBuf::from("/usr/local/share/btop/themes"));
    dirs.push(PathBuf::from("/usr/share/btop/themes"));
    locate_theme(name, &dirs)
}

/// Resolves a `color_theme` name the way btop does: builtin names are not files,
/// an absolute path is used only when that file exists, and anything else is a
/// stem or filename under the theme directories. A relative path is never joined
/// onto the working directory, or a file sitting there would shadow the theme.
pub(crate) fn locate_theme(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    if name == "Default" || name == "TTY" || name.is_empty() {
        return None;
    }
    let path = Path::new(name);
    if path.is_absolute() {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
    } else if name.contains('/') {
        return None;
    }
    let base = path.file_name().and_then(|n| n.to_str()).unwrap_or(name);
    let stem = base.strip_suffix(".theme").unwrap_or(base);
    if stem.is_empty() {
        return None;
    }
    dirs.iter()
        .map(|dir| dir.join(format!("{stem}.theme")))
        .find(|candidate| candidate.is_file())
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

    fn rgb(c: Color) -> Rgb {
        match c {
            Color::Rgb(r, g, b) => (r, g, b),
            other => panic!("expected rgb, got {other:?}"),
        }
    }

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

    #[test]
    fn empty_theme_is_btops_default_palette() {
        let t = Theme::from_str("");
        assert_eq!(rgb(t.fg), DEFAULT_FG);
        assert_eq!(rgb(t.title), DEFAULT_TITLE);
        assert_eq!(rgb(t.hi), DEFAULT_HI);
        assert_eq!(rgb(t.inactive), DEFAULT_INACTIVE);
        assert_eq!(rgb(t.graph_text), DEFAULT_GRAPH);
        assert_eq!(rgb(t.border), DEFAULT_DIV);
        assert_eq!(rgb(t.master_border), DEFAULT_NET_BOX);
        assert_eq!(rgb(t.good), DEFAULT_TEMP[0]);
        assert_eq!(rgb(t.bad), DEFAULT_TEMP[2]);
        assert_eq!(t.rx, DEFAULT_RX);
        assert_eq!(t.tx, DEFAULT_TX);
        assert_eq!(t.meter, DEFAULT_TEMP);
        assert_eq!(rgb(t.meter_off), DEFAULT_METER_BG);
    }

    #[test]
    fn missing_optional_colors_copy_inactive_fg() {
        let t = Theme::from_str("theme[inactive_fg]=\"#112233\"\n");
        assert_eq!(rgb(t.inactive), (0x11, 0x22, 0x33));
        assert_eq!(rgb(t.meter_off), (0x11, 0x22, 0x33));
        assert_eq!(rgb(t.graph_text), (0x11, 0x22, 0x33));
        assert_eq!(rgb(t.master_border), DEFAULT_NET_BOX);
    }

    #[test]
    fn theme_name_stops_at_the_closing_quote() {
        assert_eq!(theme_name_from(""), "Default");
        assert_eq!(theme_name_from("color_theme = \"nord\" # note\n"), "nord");
        assert_eq!(theme_name_from("color_theme = nord.theme\n"), "nord.theme");
        assert_eq!(theme_name_from("color_theme = \"TTY\"\n"), "TTY");
    }

    #[test]
    fn locate_ignores_a_cwd_file_and_finds_the_theme_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("cwd");
        let user = tmp.path().join("themes");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&user).unwrap();
        fs::write(cwd.join("current"), "not a theme").unwrap();
        fs::write(user.join("current.theme"), "theme[main_fg]=\"#010203\"\n").unwrap();
        fs::write(user.join("nord.theme"), "theme[main_fg]=\"#112233\"\n").unwrap();

        let dirs = [user.clone()];
        assert_eq!(
            locate_theme("current", &dirs),
            Some(user.join("current.theme"))
        );
        assert_eq!(
            locate_theme("nord.theme", &dirs),
            Some(user.join("nord.theme"))
        );
        assert_eq!(locate_theme("Default", &dirs), None);
        assert_eq!(locate_theme("TTY", &dirs), None);
        assert_eq!(locate_theme("missing", &dirs), None);

        let absolute = user.join("nord.theme");
        assert_eq!(
            locate_theme(absolute.to_str().unwrap(), &dirs),
            Some(absolute)
        );
        // An absolute path is the file itself when it exists, and never a cwd guess.
        assert_eq!(
            locate_theme(cwd.join("current").to_str().unwrap(), &dirs),
            Some(cwd.join("current"))
        );
        assert_eq!(
            locate_theme("/no/such/current.theme", &dirs),
            Some(user.join("current.theme"))
        );
    }
}
