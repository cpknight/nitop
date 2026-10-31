//! Rendering. Everything is drawn straight to the buffer to get btop's box titles
//! and two-sided braille graphs.

use crate::App;
use crate::ifinfo::Details;
use crate::meter::{Meters, Scale};
use crate::net::{Flow, Series};
use crate::theme::{Rgb, gradient};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

const MIN_BOX_WIDTH: u16 = 60;
const MIN_BOX_HEIGHT: u16 = 8;
const MIN_MASTER_HEIGHT: u16 = 6;
const TABLE_WIDTH: u16 = 30;
const MIN_CHART_WIDTH: u16 = 12;
/// Inner height at which an interface box gives its details a second line.
const DETAILS_TWO_LINES_FROM: u16 = 7;
/// Graphs never scale below this, so idle chatter doesn't look like a full-height burst.
const SCALE_FLOOR_BYTES: f64 = 10.0 * 1024.0;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let buf = f.buffer_mut();
    if area.height < 2 || area.width < 10 {
        return;
    }
    let body = Rect {
        height: area.height - 1,
        ..area
    };
    let grid = Grid::new(body, app.monitor.ifaces.len());
    app.scroll = app.scroll.min(grid.max_scroll());

    let m = &app.monitor;
    let total = m.ifaces.iter().filter(|i| !i.loopback).count();
    let up = m.ifaces.iter().filter(|i| !i.loopback && i.up).count();
    draw_box(
        buf,
        grid.master,
        app,
        &BoxSpec {
            title: "all traffic",
            info: &[(format!("{up}/{total} up, excl. lo"), app.theme.fg)],
            border: app.theme.master_border,
            series: &m.master,
            details: None,
            meters: Some(&app.meters),
        },
    );

    for (rect, iface) in grid
        .visible(app.scroll)
        .into_iter()
        .zip(m.ifaces.iter().skip(grid.first_index(app.scroll)))
    {
        let state = if iface.up {
            ("up", app.theme.good)
        } else {
            ("down", app.theme.bad)
        };
        let mut info = vec![(state.0.to_string(), state.1)];
        if let Some(s) = iface.speed {
            info.push((fmt_speed(s), app.theme.fg));
        }
        if let Some(a) = &iface.addr {
            info.push((a.clone(), app.theme.fg));
        }
        draw_box(
            buf,
            rect,
            app,
            &BoxSpec {
                title: &iface.name,
                info: &info,
                border: app.theme.border,
                series: &iface.series,
                details: iface.details.as_ref(),
                meters: None,
            },
        );
    }

    draw_footer(
        buf,
        Rect {
            y: area.height - 1,
            height: 1,
            ..area
        },
        app,
        &grid,
    );
}

/// Where everything goes for a given terminal size and interface count.
struct Grid {
    master: Rect,
    area: Rect,
    n: usize,
    cols: usize,
    total_rows: usize,
    visible_rows: usize,
}

impl Grid {
    fn new(body: Rect, n: usize) -> Self {
        let master_h = (body.height * 2 / 5)
            .max(MIN_MASTER_HEIGHT)
            .min(body.height);
        let master = Rect {
            height: master_h,
            ..body
        };
        let area = Rect {
            y: body.y + master_h,
            height: body.height - master_h,
            ..body
        };
        let cols = ((body.width / MIN_BOX_WIDTH) as usize).clamp(1, n.max(1));
        let total_rows = n.div_ceil(cols);
        // Too short for a usable box: show none, rather than slivers.
        let visible_rows = if area.height < 5 {
            0
        } else {
            ((area.height / MIN_BOX_HEIGHT) as usize)
                .clamp(1, total_rows.max(1))
                .min(total_rows)
        };
        Grid {
            master,
            area,
            n,
            cols,
            total_rows,
            visible_rows,
        }
    }

    fn max_scroll(&self) -> usize {
        self.total_rows.saturating_sub(self.visible_rows)
    }

    fn first_index(&self, scroll: usize) -> usize {
        scroll * self.cols
    }

    fn visible_count(&self, scroll: usize) -> usize {
        (self.visible_rows * self.cols).min(self.n - self.first_index(scroll).min(self.n))
    }

    /// Rects for the visible boxes, in interface order. Rows share the height evenly and
    /// a short last row stretches to the full width.
    fn visible(&self, scroll: usize) -> Vec<Rect> {
        let mut out = Vec::new();
        let first = self.first_index(scroll);
        for row in 0..self.visible_rows {
            let y0 = self.area.height as usize * row / self.visible_rows;
            let y1 = self.area.height as usize * (row + 1) / self.visible_rows;
            let start = first + row * self.cols;
            let in_row = self.cols.min(self.n.saturating_sub(start));
            for col in 0..in_row {
                let x0 = self.area.width as usize * col / in_row;
                let x1 = self.area.width as usize * (col + 1) / in_row;
                out.push(Rect {
                    x: self.area.x + x0 as u16,
                    y: self.area.y + y0 as u16,
                    width: (x1 - x0) as u16,
                    height: (y1 - y0) as u16,
                });
            }
        }
        out
    }
}

fn draw_footer(buf: &mut Buffer, area: Rect, app: &App, grid: &Grid) {
    let t = &app.theme;
    let mut x = area.x + 1;
    let mut key = |buf: &mut Buffer, k: &str, label: &str| {
        x = put(
            buf,
            x,
            area.y,
            area,
            k,
            Style::new().fg(t.hi).add_modifier(Modifier::BOLD),
        );
        x = put(
            buf,
            x,
            area.y,
            area,
            &format!(" {label}  "),
            Style::new().fg(t.fg),
        );
    };
    key(buf, "q", "quit");
    key(buf, "b", if app.bits { "bits" } else { "bytes" });
    key(buf, "+/-", &format!("{}ms", app.interval_ms()));
    if grid.max_scroll() > 0 {
        let first = grid.first_index(app.scroll);
        let shown = format!(
            "{}-{}/{}",
            first + 1,
            first + grid.visible_count(app.scroll),
            grid.n
        );
        key(buf, "↑↓", &shown);
    }
}

struct BoxSpec<'a> {
    title: &'a str,
    info: &'a [(String, Color)],
    border: Color,
    series: &'a Series,
    details: Option<&'a Details>,
    meters: Option<&'a Meters>,
}

fn draw_box(buf: &mut Buffer, r: Rect, app: &App, spec: &BoxSpec) {
    if r.width < 12 || r.height < 3 {
        return;
    }
    draw_frame(buf, r, app, spec.title, spec.info, spec.border);
    let mut inner = Rect {
        x: r.x + 1,
        y: r.y + 1,
        width: r.width - 2,
        height: r.height - 2,
    };
    if let Some(d) = spec.details {
        let max_lines = if inner.height >= DETAILS_TWO_LINES_FROM {
            2
        } else if inner.height >= 4 {
            1
        } else {
            0
        };
        let used = draw_details(buf, inner, app, d, max_lines);
        inner.y += used;
        inner.height -= used;
    }
    let series = spec.series;
    if inner.width >= TABLE_WIDTH + 1 + MIN_CHART_WIDTH {
        let left = Rect {
            width: inner.width - TABLE_WIDTH - 1,
            ..inner
        };
        let table = Rect {
            x: inner.right() - TABLE_WIDTH,
            width: TABLE_WIDTH,
            ..inner
        };
        draw_graphs(buf, left, app, series, spec.meters);
        draw_table(buf, table, app, series);
    } else if inner.width >= TABLE_WIDTH {
        draw_table(buf, inner, app, series);
    } else {
        draw_graphs(buf, inner, app, series, spec.meters);
    }
}

/// The history chart, with the level meter beneath it when there's one and room for it.
fn draw_graphs(buf: &mut Buffer, r: Rect, app: &App, series: &Series, meters: Option<&Meters>) {
    let meter_rows = match (meters, r.height) {
        (Some(_), 7..) => 3,
        (Some(_), 4..) => 2,
        _ => 0,
    };
    let chart = Rect {
        height: r.height - meter_rows,
        ..r
    };
    draw_chart(buf, chart, app, series);
    if let Some(m) = meters.filter(|_| meter_rows > 0) {
        let area = Rect {
            y: chart.bottom(),
            height: meter_rows,
            ..r
        };
        draw_meters(buf, area, app, m);
    }
}

/// Two stereo-style LED bars, with the shared scale between them when it fits.
fn draw_meters(buf: &mut Buffer, r: Rect, app: &App, m: &Meters) {
    const PREFIX: u16 = 2;
    if r.width <= PREFIX {
        return;
    }
    let t = &app.theme;
    let scale = Scale::new(app.bits);
    let n = (r.width - PREFIX) as usize;
    let label = Style::new().fg(t.graph_text);
    let bar = |buf: &mut Buffer, y: u16, arrow: &str, ch: &crate::meter::Meter| {
        put(buf, r.x, y, r, arrow, label);
        let lit = scale.segments(ch.level, n);
        let peak = scale.segments(ch.peak, n);
        for i in 0..n {
            let on = i < lit || i + 1 == peak;
            let color = if on {
                gradient(&t.meter, (i as f64 + 0.5) / n as f64)
            } else {
                t.meter_off
            };
            let cell = &mut buf[(r.x + PREFIX + i as u16, y)];
            cell.set_char('■');
            cell.set_style(Style::new().fg(color));
        }
    };
    bar(buf, r.y, "▼ ", &m.rx);
    bar(buf, r.bottom() - 1, "▲ ", &m.tx);
    if r.height >= 3 {
        draw_meter_scale(
            buf,
            Rect {
                x: r.x + PREFIX,
                y: r.y + 1,
                width: r.width - PREFIX,
                height: 1,
            },
            &scale,
            label,
        );
    }
}

/// Tick labels at their true positions, skipping any that would collide. The last carries
/// the unit and is right-aligned so it always fits.
fn draw_meter_scale(buf: &mut Buffer, r: Rect, scale: &Scale, style: Style) {
    let n = r.width as usize;
    let Some(((_, last), rest)) = scale.ticks.split_last() else {
        return;
    };
    let last = format!("{last} {}", scale.unit);
    let last_x = n.saturating_sub(last.chars().count());
    put(buf, r.x + last_x as u16, r.y, r, &last, style);
    let mut free = 0;
    for &(value, text) in rest {
        let x = scale.segments(value.log10(), n).saturating_sub(1);
        if x >= free && x + text.len() < last_x {
            put(buf, r.x + x as u16, r.y, r, text, style);
            free = x + text.len() + 1;
        }
    }
}

/// One detail about an interface. Order is priority: what doesn't fit is dropped from the end.
struct Fact {
    label: &'static str,
    value: String,
    color: Option<Color>,
}

fn facts(d: &Details, app: &App) -> Vec<Fact> {
    let t = &app.theme;
    let mut out = Vec::new();
    let mut push = |label, value: String, color| {
        out.push(Fact {
            label,
            value,
            color,
        })
    };
    push("", d.kind.clone(), Some(t.hi));
    if d.kind == "Wi-Fi" {
        match &d.wifi {
            Some(w) => {
                match &w.ssid {
                    Some(ssid) => push("SSID", ssid.clone(), Some(t.title)),
                    None if w.signal_dbm.is_none() => {
                        push("", "not connected".into(), Some(t.inactive))
                    }
                    None => push("", "hidden SSID".into(), Some(t.inactive)),
                }
                if let Some(s) = w.signal_dbm {
                    let c = if s >= -60 {
                        t.good
                    } else if s <= -75 {
                        t.bad
                    } else {
                        t.fg
                    };
                    push("", format!("{s} dBm"), Some(c));
                }
                if let Some(b) = w.band() {
                    push("", b, None);
                }
                if let Some(g) = w.generation {
                    push("", g.into(), None);
                }
                if let Some(r) = w.tx_mbps {
                    push("", format!("{r:.0} Mbps"), None);
                }
            }
            None => push("", "link info unavailable".into(), Some(t.inactive)),
        }
    }
    if let Some(h) = &d.hardware {
        push("", h.clone(), None);
    }
    if let Some(v) = &d.driver {
        push("driver", v.clone(), None);
    }
    if let Some(v) = &d.duplex {
        push("", format!("{v} duplex"), None);
    }
    if let Some(v) = &d.mac {
        push("MAC", v.clone(), None);
    }
    if let Some(v) = d.mtu {
        push("MTU", v.to_string(), None);
    }
    out
}

/// Flows facts into up to `max_lines` rows; returns how many rows it used. A fact too long
/// for a whole row (a long hardware name) is truncated rather than lost.
fn draw_details(buf: &mut Buffer, r: Rect, app: &App, d: &Details, max_lines: u16) -> u16 {
    const GAP: usize = 2;
    let t = &app.theme;
    let width = r.width as usize;
    let mut line = 0u16;
    let mut used = 0usize;
    for fact in facts(d, app) {
        if max_lines == 0 {
            break;
        }
        let label_len = if fact.label.is_empty() {
            0
        } else {
            fact.label.len() + 1
        };
        let value = truncate(&fact.value, width.saturating_sub(label_len));
        let len = label_len + value.chars().count();
        let start = if used == 0 { 0 } else { used + GAP };
        let (line_now, x) = if start + len <= width {
            (line, start)
        } else if line + 1 < max_lines {
            (line + 1, 0)
        } else {
            continue; // a shorter, later fact may still fit
        };
        line = line_now;
        let y = r.y + line;
        let mut cx = r.x + x as u16;
        if label_len > 0 {
            cx = put(buf, cx, y, r, fact.label, Style::new().fg(t.inactive));
            cx += 1;
        }
        put(
            buf,
            cx,
            y,
            r,
            &value,
            Style::new().fg(fact.color.unwrap_or(t.fg)),
        );
        used = x + len;
    }
    if max_lines == 0 { 0 } else { line + 1 }
}

/// Rounded border with btop's `┐title┌` tabs: title on the left, facts on the right.
fn draw_frame(
    buf: &mut Buffer,
    r: Rect,
    app: &App,
    title: &str,
    info: &[(String, Color)],
    border: Color,
) {
    let bs = Style::new().fg(border);
    let (right, bottom) = (r.right() - 1, r.bottom() - 1);
    for x in r.x..=right {
        for y in [r.y, bottom] {
            buf[(x, y)].set_symbol("─").set_style(bs);
        }
    }
    for y in r.y..=bottom {
        for x in [r.x, right] {
            buf[(x, y)].set_symbol("│").set_style(bs);
        }
    }
    buf[(r.x, r.y)].set_symbol("╭");
    buf[(right, r.y)].set_symbol("╮");
    buf[(r.x, bottom)].set_symbol("╰");
    buf[(right, bottom)].set_symbol("╯");

    let clip = Rect {
        x: r.x + 1,
        width: r.width - 2,
        height: 1,
        y: r.y,
    };
    // Keep the right corner's run of line clear of the title tab.
    let title = truncate(title, r.width.saturating_sub(6) as usize);
    let mut x = put(buf, r.x + 1, r.y, clip, "┐", bs);
    x = put(
        buf,
        x,
        r.y,
        clip,
        &title,
        Style::new()
            .fg(app.theme.title)
            .add_modifier(Modifier::BOLD),
    );
    let title_end = put(buf, x, r.y, clip, "┌", bs);

    // Each fact takes its own tab, and facts that don't fit are dropped from the end.
    let mut used = 0usize;
    let mut fit = 0;
    let room = (right - 1).saturating_sub(title_end + 1) as usize;
    for (text, _) in info {
        let need = text.chars().count() + 2;
        if used + need > room {
            break;
        }
        used += need;
        fit += 1;
    }
    let mut x = right - used as u16;
    for (text, color) in &info[..fit] {
        x = put(buf, x, r.y, clip, "┐", bs);
        x = put(buf, x, r.y, clip, text, Style::new().fg(*color));
        x = put(buf, x, r.y, clip, "┌", bs);
    }
}

/// Inbound grows up from the middle, outbound grows down, each on its own scale
/// (labelled), because in and out often differ by orders of magnitude.
fn draw_chart(buf: &mut Buffer, r: Rect, app: &App, series: &Series) {
    if r.height < 2 || r.width == 0 {
        return;
    }
    let rx_rows = r.height.div_ceil(2);
    let tx_rows = r.height - rx_rows;
    let rx_area = Rect {
        height: rx_rows,
        ..r
    };
    let tx_area = Rect {
        y: r.y + rx_rows,
        height: tx_rows,
        ..r
    };
    let t = &app.theme;
    draw_half(
        buf,
        rx_area,
        &series.rx,
        app.bits,
        &t.rx,
        true,
        t.graph_text,
    );
    draw_half(
        buf,
        tx_area,
        &series.tx,
        app.bits,
        &t.tx,
        false,
        t.graph_text,
    );
}

fn draw_half(
    buf: &mut Buffer,
    r: Rect,
    flow: &Flow,
    bits: bool,
    colors: &[Rgb; 3],
    up: bool,
    label: Color,
) {
    let cols = r.width as usize;
    let dots = cols * 2;
    let skip = flow.history.len().saturating_sub(dots);
    let visible: Vec<f64> = flow.history.iter().skip(skip).copied().collect();
    let scale = auto_scale(visible.iter().copied().fold(0.0, f64::max), bits);
    let max_dots = r.height as usize * 4;

    // Right-aligned: the newest sample is always at the right edge.
    let pad = dots - visible.len();
    let height_at = |i: usize| -> usize {
        i.checked_sub(pad)
            .map_or(0, |i| dot_height(visible[i], scale, max_dots))
    };

    for col in 0..cols {
        let (hl, hr) = (height_at(col * 2), height_at(col * 2 + 1));
        for row in 0..r.height as usize {
            // `row` counts away from the middle of the chart.
            let fl = hl.saturating_sub(row * 4).min(4);
            let fr = hr.saturating_sub(row * 4).min(4);
            if fl == 0 && fr == 0 {
                continue;
            }
            let y = if up {
                r.bottom() - 1 - row as u16
            } else {
                r.y + row as u16
            };
            let t = (row as f64 + 0.5) / r.height as f64;
            let cell = &mut buf[(r.x + col as u16, y)];
            cell.set_char(braille(fl, fr, up));
            cell.set_fg(gradient(colors, t));
        }
    }

    let arrow = if up { "▼" } else { "▲" };
    let text = format!("{arrow} {}", fmt_rate(scale, bits));
    let y = if up { r.y } else { r.bottom() - 1 };
    put(buf, r.x, y, r, &text, Style::new().fg(label));
}

/// Dots lit for a value. Any nonzero traffic gets at least one, so it is never invisible.
fn dot_height(v: f64, scale: f64, max: usize) -> usize {
    if v <= 0.0 || scale <= 0.0 {
        return 0;
    }
    ((v / scale * max as f64).round() as usize).clamp(1, max)
}

/// Braille cell with `l`/`r` dots filled per column, from the bottom if `up`, else the top.
fn braille(l: usize, r: usize, up: bool) -> char {
    const LEFT: [u32; 4] = [0x01, 0x02, 0x04, 0x40]; // top to bottom
    const RIGHT: [u32; 4] = [0x08, 0x10, 0x20, 0x80];
    let mut bits = 0;
    for i in 0..4 {
        let row = if up { 3 - i } else { i };
        if l > i {
            bits |= LEFT[row];
        }
        if r > i {
            bits |= RIGHT[row];
        }
    }
    char::from_u32(0x2800 + bits).unwrap_or(' ')
}

/// Smallest 1-2-5 step in the display unit (bytes/s or bits/s) that holds `max`.
/// Returned in bytes/s. Round values keep the label stable as traffic moves.
fn auto_scale(max_bytes: f64, bits: bool) -> f64 {
    let (factor, base) = if bits { (8.0, 1000.0) } else { (1.0, 1024.0) };
    let target = max_bytes.max(SCALE_FLOOR_BYTES) * factor;
    let mut unit = 1.0_f64;
    loop {
        for m in [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0] {
            if m * unit >= target {
                return m * unit / factor;
            }
        }
        unit *= base;
    }
}

fn draw_table(buf: &mut Buffer, r: Rect, app: &App, s: &Series) {
    let t = &app.theme;
    let c = &s.counters;
    let bits = app.bits;
    let label = Style::new().fg(t.fg);
    let rx = Style::new().fg(gradient(&t.rx, 0.5));
    let tx = Style::new().fg(gradient(&t.tx, 0.5));
    let count = |n: u64| Style::new().fg(if n > 0 { t.bad } else { t.inactive });

    // In priority order: rows that don't fit are dropped from the end.
    let rows: [(&str, String, Style, String, Style); 8] = [
        (
            "Rate",
            fmt_rate(s.rx.rate, bits),
            rx.add_modifier(Modifier::BOLD),
            fmt_rate(s.tx.rate, bits),
            tx.add_modifier(Modifier::BOLD),
        ),
        (
            "Peak",
            fmt_rate(s.rx.peak, bits),
            rx,
            fmt_rate(s.tx.peak, bits),
            tx,
        ),
        (
            "Avg",
            fmt_rate(s.avg(&s.rx), bits),
            rx,
            fmt_rate(s.avg(&s.tx), bits),
            tx,
        ),
        (
            "Session",
            fmt_bytes(s.rx.session),
            rx,
            fmt_bytes(s.tx.session),
            tx,
        ),
        (
            "Total",
            fmt_bytes(c.rx_bytes),
            rx,
            fmt_bytes(c.tx_bytes),
            tx,
        ),
        (
            "Pkts/s",
            fmt_count(s.rx.packet_rate),
            rx,
            fmt_count(s.tx.packet_rate),
            tx,
        ),
        (
            "Errors",
            c.rx_errs.to_string(),
            count(c.rx_errs),
            c.tx_errs.to_string(),
            count(c.tx_errs),
        ),
        (
            "Drops",
            c.rx_drop.to_string(),
            count(c.rx_drop),
            c.tx_drop.to_string(),
            count(c.tx_drop),
        ),
    ];

    let header = Style::new().fg(t.title).add_modifier(Modifier::BOLD);
    table_row(buf, r, r.y, "", "▼ In", header, "▲ Out", header, label);
    for (i, (name, a, sa, b, sb)) in rows.iter().enumerate() {
        let y = r.y + 1 + i as u16;
        if y >= r.bottom() {
            break;
        }
        table_row(buf, r, y, name, a, *sa, b, *sb, label);
    }
}

#[allow(clippy::too_many_arguments)]
fn table_row(
    buf: &mut Buffer,
    r: Rect,
    y: u16,
    name: &str,
    a: &str,
    sa: Style,
    b: &str,
    sb: Style,
    label: Style,
) {
    const VALUE: u16 = 11;
    put(buf, r.x, y, r, name, label);
    let cell = |s: &str| format!("{:>w$}", truncate(s, VALUE as usize), w = VALUE as usize);
    put(buf, r.right() - 2 * VALUE - 1, y, r, &cell(a), sa);
    put(buf, r.right() - VALUE, y, r, &cell(b), sb);
}

/// Writes `s` clipped to `clip`, returning the x after it.
fn put(buf: &mut Buffer, x: u16, y: u16, clip: Rect, s: &str, style: Style) -> u16 {
    let mut x = x;
    for ch in s.chars() {
        if x >= clip.right() || y >= clip.bottom() {
            break;
        }
        let cell = &mut buf[(x, y)];
        cell.set_char(ch);
        cell.set_style(style);
        x += 1;
    }
    x
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Three significant digits, wide enough to keep the table from jittering.
fn sig(v: f64) -> String {
    if v < 10.0 {
        format!("{v:.2}")
    } else if v < 100.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    }
}

fn scaled(mut v: f64, base: f64, units: &[&str]) -> String {
    let mut i = 0;
    while v >= base && i + 1 < units.len() {
        v /= base;
        i += 1;
    }
    if i == 0 {
        format!("{v:.0} {}", units[0])
    } else {
        format!("{} {}", sig(v), units[i])
    }
}

/// Rate for display: binary units for bytes, decimal for bits, as is conventional.
pub fn fmt_rate(bytes_per_sec: f64, bits: bool) -> String {
    if bits {
        scaled(
            bytes_per_sec * 8.0,
            1000.0,
            &["bps", "Kbps", "Mbps", "Gbps", "Tbps"],
        )
    } else {
        scaled(
            bytes_per_sec,
            1024.0,
            &["B/s", "KiB/s", "MiB/s", "GiB/s", "TiB/s"],
        )
    }
}

pub fn fmt_bytes(b: u64) -> String {
    scaled(b as f64, 1024.0, &["B", "KiB", "MiB", "GiB", "TiB", "PiB"])
}

fn fmt_count(v: f64) -> String {
    scaled(v, 1000.0, &["", "K", "M", "G"])
        .trim_end()
        .to_string()
}

fn fmt_speed(mbps: u32) -> String {
    if mbps >= 1000 {
        format!("{}G", mbps as f64 / 1000.0)
    } else {
        format!("{mbps}M")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_units() {
        assert_eq!(fmt_rate(0.0, false), "0 B/s");
        assert_eq!(fmt_rate(1536.0, false), "1.50 KiB/s");
        assert_eq!(fmt_rate(125_000.0, true), "1.00 Mbps");
        assert_eq!(fmt_bytes(3 * 1024 * 1024 * 1024), "3.00 GiB");
        assert_eq!(fmt_count(12.0), "12");
        assert_eq!(fmt_count(1500.0), "1.50 K");
        assert_eq!(fmt_speed(2500), "2.5G");
    }

    #[test]
    fn longest_rates_fit_the_table_column() {
        for v in [1023.9_f64, 1023.9 * 1024.0, 999.9e9] {
            assert!(fmt_rate(v, false).chars().count() <= 11);
            assert!(fmt_rate(v, true).chars().count() <= 11);
        }
    }

    #[test]
    fn braille_fills_from_the_baseline() {
        assert_eq!(braille(0, 0, true), '\u{2800}');
        assert_eq!(braille(1, 0, true), '\u{2840}'); // bottom-left dot
        assert_eq!(braille(0, 1, false), '\u{2808}'); // top-right dot
        assert_eq!(braille(4, 4, true), '\u{28ff}');
        assert_eq!(braille(4, 4, false), '\u{28ff}');
    }

    #[test]
    fn nonzero_traffic_is_always_visible_and_zero_is_not() {
        assert_eq!(dot_height(0.0, 100.0, 8), 0);
        assert_eq!(dot_height(0.001, 100.0, 8), 1);
        assert_eq!(dot_height(100.0, 100.0, 8), 8);
        assert_eq!(dot_height(500.0, 100.0, 8), 8);
    }

    #[test]
    fn scale_is_round_holds_the_max_and_has_a_floor() {
        assert_eq!(auto_scale(0.0, false), 10.0 * 1024.0);
        assert_eq!(
            auto_scale(1.5 * 1024.0 * 1024.0, false),
            2.0 * 1024.0 * 1024.0
        );
        assert_eq!(auto_scale(100_000.0, true), 1_000_000.0 / 8.0); // 800 Kbps -> 1 Mbps
    }

    #[test]
    fn grid_never_leaves_the_body_at_any_size() {
        for w in [10u16, 40, 59, 60, 80, 120, 200, 400] {
            for h in [1u16, 5, 8, 14, 24, 50, 100] {
                for n in [0usize, 1, 2, 3, 7, 40] {
                    let body = Rect {
                        x: 0,
                        y: 0,
                        width: w,
                        height: h,
                    };
                    let g = Grid::new(body, n);
                    assert!(g.master.bottom() <= h);
                    for scroll in 0..=g.max_scroll() {
                        let rects = g.visible(scroll);
                        assert_eq!(rects.len(), g.visible_count(scroll), "{w}x{h} n={n}");
                        for r in rects {
                            assert!(r.right() <= w && r.bottom() <= h, "{w}x{h} n={n} {r:?}");
                            assert!(r.width > 0 && r.height > 0);
                        }
                    }
                }
            }
        }
    }

    fn test_app(monitor: crate::net::Monitor) -> crate::App {
        crate::App::new(monitor, crate::theme::Theme::from_str(""))
    }

    fn wifi_iface() -> crate::net::Iface {
        crate::net::Iface {
            name: "wlan0".into(),
            series: Series::default(),
            loopback: false,
            up: true,
            addr: None,
            speed: None,
            details: Some(Details {
                kind: "Wi-Fi".into(),
                hardware: Some("Intel Wi-Fi 6E AX211 160MHz".into()),
                driver: Some("iwlwifi".into()),
                mac: Some("02:00:00:00:00:01".into()),
                mtu: Some(1500),
                duplex: None,
                wifi: Some(crate::wifi::WifiLink {
                    ssid: Some("homenet".into()),
                    freq_mhz: Some(5220),
                    width_mhz: Some(80),
                    signal_dbm: Some(-59),
                    tx_mbps: Some(866.0),
                    generation: Some("Wi-Fi 6"),
                }),
            }),
        }
    }

    fn render(app: &mut crate::App, w: u16, h: u16) -> Vec<String> {
        use ratatui::{Terminal, backend::TestBackend};
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect()
    }

    #[test]
    fn interface_box_shows_wifi_details() {
        let mut m = crate::net::Monitor::default();
        m.ifaces.push(wifi_iface());
        let text = render(&mut test_app(m), 100, 40).join("\n");
        for want in [
            "Wi-Fi",
            "SSID homenet",
            "-59 dBm",
            "5 GHz ch44 80 MHz",
            "866 Mbps",
            "Intel Wi-Fi 6E AX211",
            "driver iwlwifi",
            "MAC 02:00",
        ] {
            assert!(text.contains(want), "missing {want:?} in:\n{text}");
        }
    }

    #[test]
    fn narrow_box_keeps_details_in_bounds_and_truncates_hardware() {
        let mut m = crate::net::Monitor::default();
        m.ifaces.push(wifi_iface());
        let lines = render(&mut test_app(m), 24, 40);
        let text = lines.join("\n");
        assert!(text.contains("Wi-Fi"), "{text}");
        assert!(lines.iter().all(|l| l.chars().count() == 24));
    }

    #[test]
    fn meter_lights_segments_only_in_master_box() {
        let mut m = crate::net::Monitor::default();
        m.ifaces.push(wifi_iface());
        let mut app = test_app(m);
        app.meters.rx.feed(10.0 * 1024.0 * 1024.0);
        let lines = render(&mut app, 100, 40);
        let text = lines.join("\n");
        assert!(text.contains("▼ ■■■"), "{text}");
        assert!(text.contains("10G B/s"), "{text}");
        let master_end = lines.iter().position(|l| l.starts_with('╰')).unwrap();
        assert!(lines[master_end..].iter().all(|l| !l.contains('■')));
    }

    #[test]
    fn peak_marker_sits_apart_from_the_bar() {
        let mut m = crate::net::Monitor::default();
        m.ifaces.push(wifi_iface());
        let mut app = test_app(m);
        app.meters.rx.feed(1024.0 * 1024.0 * 1024.0);
        app.meters.rx.feed(2048.0);
        app.meters.rx.tick(5.0);
        let scale = Scale::new(false);
        assert!(
            scale.segments(app.meters.rx.peak, 50) > scale.segments(app.meters.rx.level, 50) + 1
        );
    }

    #[test]
    fn full_render_never_panics_at_any_size() {
        use ratatui::{Terminal, backend::TestBackend};
        let (monitor, _) = crate::net::Monitor::new().unwrap();
        let mut app = crate::App::new(monitor, crate::theme::Theme::load());
        app.scroll = usize::MAX;
        for w in [1u16, 9, 10, 12, 13, 30, 42, 44, 60, 80, 200] {
            for h in [1u16, 2, 3, 4, 5, 8, 12, 24, 60] {
                let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                term.draw(|f| draw(f, &mut app)).unwrap();
            }
        }
    }
}
