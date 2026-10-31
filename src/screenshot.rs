//! Renders the real UI with made-up interfaces to an SVG, so the README's screenshot shows
//! no one's actual network. Regenerate as described in CONTRIBUTING.md.

use crate::App;
use crate::ifinfo::Details;
use crate::net::{Counters, Monitor};
use crate::wifi::WifiLink;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use ratatui::{Terminal, backend::TestBackend};
use std::fmt::Write;

const CW: f64 = 9.0;
const CH: f64 = 18.0;
const BG: &str = "#1a1b26";
const THEME: &str = r##"
theme[main_fg]="#a9b1d6"
theme[title]="#c0caf5"
theme[hi_fg]="#7aa2f7"
theme[inactive_fg]="#565f89"
theme[graph_text]="#b4bee6"
theme[meter_bg]="#292e42"
theme[net_box]="#f7768e"
theme[div_line]="#414868"
theme[temp_start]="#9ece6a"
theme[temp_mid]="#e0af68"
theme[temp_end]="#f7768e"
theme[download_start]="#e0af68"
theme[download_mid]="#f7768e"
theme[download_end]="#f7768e"
theme[upload_start]="#9ece6a"
theme[upload_mid]="#449dab"
theme[upload_end]="#7aa2f7"
"##;

/// Repeatable traffic with bursts: a small generator so that runs are identical.
struct Traffic(u64);

impl Traffic {
    fn next(&mut self, base: f64, burst: f64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let r = (self.0 >> 33) as f64 / (1u64 << 31) as f64;
        let swell = if r > 0.8 { burst } else { 1.0 + 2.0 * r };
        (base * swell) as u64
    }
}

fn demo_app() -> App {
    let names = ["eth0", "wlan0", "wg0", "lo"];
    // (name, inbound base, inbound burst, outbound base, outbound burst)
    let load = [
        (1.4e6, 14.0, 9e4, 30.0),
        (3.0e5, 22.0, 4e4, 12.0),
        (6.0e4, 18.0, 2e4, 15.0),
        (2.0e4, 6.0, 2e4, 6.0),
    ];
    let mut rng = Traffic(42);
    let mut m = Monitor::default();
    let mut totals = [Counters::default(); 4];
    let mut last = (0.0, 0.0);
    // Real traffic has momentum, so each direction drifts toward its latest draw.
    let mut smooth = [[0.0_f64; 2]; 4];
    for t in 0..240 {
        let mut rows = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let (rb, rx, tb, tx) = load[i];
            let c = &mut totals[i];
            // Interfaces get busier toward the right, so the charts have some shape.
            let wave = 1.0 + (t as f64 / 17.0 + i as f64).sin().abs() * 2.0;
            smooth[i][0] += (rng.next(rb, rx) as f64 * wave - smooth[i][0]) * 0.35;
            c.rx_bytes += smooth[i][0] as u64;
            smooth[i][1] += (rng.next(tb, tx) as f64 * wave - smooth[i][1]) * 0.35;
            c.tx_bytes += smooth[i][1] as u64;
            c.rx_packets += c.rx_bytes % 900 + 40;
            c.tx_packets += c.tx_bytes % 500 + 20;
            rows.push((name.to_string(), *c));
        }
        m.apply(rows, if t == 0 { 0.0 } else { 1.0 });
        let high = m
            .master
            .rx
            .history
            .iter()
            .rev()
            .take(4)
            .fold(0.0, |a: f64, &b| a.max(b));
        last = (high, m.master.tx.rate);
    }
    for i in &mut m.ifaces {
        i.loopback = i.name == "lo";
        i.up = true;
        i.details = Some(details(&i.name));
        (i.addr, i.speed) = match i.name.as_str() {
            "eth0" => (Some("192.0.2.10".to_string()), Some(2500)),
            "wlan0" => (Some("192.0.2.42".to_string()), None),
            "wg0" => (Some("198.51.100.7".to_string()), None),
            _ => (Some("127.0.0.1".to_string()), None),
        };
    }
    m.ifaces
        .sort_by(|a, b| (a.loopback, &a.name).cmp(&(b.loopback, &b.name)));
    let mut app = App::new(m, crate::theme::Theme::from_str(THEME));
    app.meters.rx.feed(last.0);
    app.meters.rx.feed(app.monitor.master.rx.rate);
    app.meters.tx.feed(app.monitor.master.tx.rate * 4.0);
    app.meters.tx.feed(app.monitor.master.tx.rate);
    app.meters.rx.tick(0.25);
    app.meters.tx.tick(0.25);
    app
}

fn details(name: &str) -> Details {
    let mut d = Details {
        mtu: Some(1500),
        ..Default::default()
    };
    match name {
        "eth0" => {
            d.kind = "Ethernet".into();
            d.hardware = Some("Realtek Semiconductor RTL8125 2.5GbE Controller".into());
            d.driver = Some("r8169".into());
            d.mac = Some("02:00:00:00:00:01".into());
            d.duplex = Some("full".into());
        }
        "wlan0" => {
            d.kind = "Wi-Fi".into();
            d.hardware = Some("Intel Wi-Fi 6E AX211 160MHz".into());
            d.driver = Some("iwlwifi".into());
            d.mac = Some("02:00:00:00:00:02".into());
            d.wifi = Some(WifiLink {
                ssid: Some("example-net".into()),
                freq_mhz: Some(5220),
                width_mhz: Some(80),
                signal_dbm: Some(-52),
                tx_mbps: Some(866.7),
                generation: Some("Wi-Fi 6"),
            });
        }
        "wg0" => {
            d.kind = "WireGuard VPN".into();
            d.mtu = Some(1420);
        }
        _ => {
            d.kind = "Loopback".into();
            d.mtu = Some(65536);
        }
    }
    d
}

fn hex(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => "#a9b1d6".into(),
    }
}

/// Draws a cell's symbol as shapes where a font's glyphs wouldn't line up across cells.
/// Returns false if it should be drawn as text instead.
fn shape(out: &mut String, sym: &str, x: f64, y: f64, fill: &str) -> bool {
    let (mx, my) = (x + CW / 2.0, y + CH / 2.0);
    let (rt, bt) = (x + CW, y + CH);
    let mut path = |d: String| {
        let _ = write!(
            out,
            r#"<path d="{d}" stroke="{fill}" stroke-width="1.2" fill="none"/>"#
        );
    };
    match sym {
        "─" => path(format!("M{x} {my}H{rt}")),
        "│" => path(format!("M{mx} {y}V{bt}")),
        "╭" => path(format!(
            "M{rt} {my}H{}Q{mx} {my} {mx} {}V{bt}",
            mx + 4.0,
            my + 4.0
        )),
        "╮" => path(format!(
            "M{x} {my}H{}Q{mx} {my} {mx} {}V{bt}",
            mx - 4.0,
            my + 4.0
        )),
        "╰" => path(format!(
            "M{rt} {my}H{}Q{mx} {my} {mx} {}V{y}",
            mx + 4.0,
            my - 4.0
        )),
        "╯" => path(format!(
            "M{x} {my}H{}Q{mx} {my} {mx} {}V{y}",
            mx - 4.0,
            my - 4.0
        )),
        "┐" => path(format!("M{x} {my}H{mx}V{bt}")),
        "┌" => path(format!("M{rt} {my}H{mx}V{bt}")),
        "■" => {
            let _ = write!(
                out,
                r#"<rect x="{}" y="{}" width="7" height="12" fill="{fill}"/>"#,
                x + 1.0,
                y + 3.0
            );
        }
        "▼" | "▲" => {
            let (a, b) = if sym == "▼" {
                (y + 5.0, y + 13.0)
            } else {
                (y + 13.0, y + 5.0)
            };
            let _ = write!(
                out,
                r#"<path d="M{} {a}H{}L{mx} {b}Z" fill="{fill}"/>"#,
                x + 1.0,
                rt - 1.0
            );
        }
        _ => {
            let Some(c) = sym
                .chars()
                .next()
                .filter(|c| ('\u{2801}'..='\u{28ff}').contains(c))
            else {
                return false;
            };
            let bits = c as u32 - 0x2800;
            // (bit, column, row) for each braille dot.
            for (bit, col, row) in [
                (0x01, 0, 0),
                (0x02, 0, 1),
                (0x04, 0, 2),
                (0x40, 0, 3),
                (0x08, 1, 0),
                (0x10, 1, 1),
                (0x20, 1, 2),
                (0x80, 1, 3),
            ] {
                if bits & bit != 0 {
                    let (cx, cy) = (x + 2.5 + col as f64 * 4.0, y + 2.25 + row as f64 * 4.5);
                    let _ = write!(
                        out,
                        r#"<circle cx="{cx}" cy="{cy}" r="1.5" fill="{fill}"/>"#
                    );
                }
            }
        }
    }
    true
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn to_svg(buf: &Buffer, w: u16, h: u16) -> String {
    let (pw, ph) = (w as f64 * CW, h as f64 * CH);
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{pw}" height="{ph}" viewBox="0 0 {pw} {ph}" font-family="DejaVu Sans Mono, Menlo, Consolas, monospace" font-size="14"><rect width="{pw}" height="{ph}" fill="{BG}"/>"#
    );
    for y in 0..h {
        let py = y as f64 * CH;
        let mut x = 0;
        while x < w {
            let cell = &buf[(x, y)];
            let fill = hex(cell.fg);
            let px = x as f64 * CW;
            if cell.symbol() == " " || shape(&mut out, cell.symbol(), px, py, &fill) {
                x += 1;
                continue;
            }
            // A run of same-styled text, fixed to the cell grid whatever font is used.
            let start = x;
            let style = (cell.fg, cell.modifier);
            let mut text = String::new();
            while x < w {
                let c = &buf[(x, y)];
                let sym = c.symbol();
                let shaped = shape(&mut String::new(), sym, 0.0, 0.0, "");
                if (c.fg, c.modifier) != style || (sym != " " && shaped) {
                    break;
                }
                text.push_str(sym);
                x += 1;
            }
            let trimmed = text.trim_end();
            let weight = if style.1.contains(Modifier::BOLD) {
                r#" font-weight="bold""#
            } else {
                ""
            };
            let _ = write!(
                out,
                r#"<text x="{}" y="{}" fill="{fill}" textLength="{}" lengthAdjust="spacingAndGlyphs" xml:space="preserve"{weight}>{}</text>"#,
                start as f64 * CW,
                py + 13.5,
                trimmed.chars().count() as f64 * CW,
                escape(trimmed),
            );
        }
    }
    out.push_str("</svg>\n");
    out
}

#[test]
#[ignore = "writes target/screenshot.svg; see CONTRIBUTING.md"]
fn generate_screenshot() {
    let (w, h) = (130, 40);
    let mut app = demo_app();
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let svg = to_svg(term.backend().buffer(), w, h);
    std::fs::create_dir_all("target").unwrap();
    std::fs::write("target/screenshot.svg", svg).unwrap();
}

#[test]
fn demo_renders_to_valid_looking_svg_with_only_made_up_data() {
    let (w, h) = (130, 40);
    let mut app = demo_app();
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let svg = to_svg(term.backend().buffer(), w, h);
    assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
    assert!(svg.contains("example-net"));
    assert!(svg.contains("<circle"), "braille should be drawn as dots");
    assert_eq!(svg.matches("<text").count(), svg.matches("</text>").count());
}
