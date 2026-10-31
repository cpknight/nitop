//! Rendering. Everything is drawn straight to the buffer to get btop's box titles
//! and two-sided braille graphs.

use crate::App;
use crate::ifinfo::Details;
use crate::meter::{Meters, Scale};
use crate::net::{Flow, Series};
use crate::theme::{Rgb, gradient};
use crate::zerotier::Info;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

const MIN_BOX_WIDTH: u16 = 60;
/// Room for four detail rows and a six-row chart and table.
const MIN_BOX_HEIGHT: u16 = 12;
const MAX_COLUMNS: usize = 3;
const MIN_MASTER_HEIGHT: u16 = 6;
const TABLE_WIDTH: u16 = 30;
const MIN_CHART_WIDTH: u16 = 12;
/// Rows the chart and table keep before detail rows take any more of a box.
const GRAPH_MIN_ROWS: u16 = 6;
const DETAIL_LABEL_WIDTH: u16 = 6;
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
    app.page = app.page.min(grid.pages() - 1);

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

    let shown = grid.rects(app.page);
    for (rect, iface) in shown
        .into_iter()
        .zip(m.ifaces.iter().skip(grid.first_index(app.page)))
    {
        let state = if iface.up {
            ("up", app.theme.good)
        } else {
            ("down", app.theme.bad)
        };
        draw_box(
            buf,
            rect,
            app,
            &BoxSpec {
                title: &iface.name,
                info: &[(state.0.to_string(), state.1)],
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

/// Where everything goes for a given terminal size and interface count: the rollup across
/// the top, then pages of identically sized interface boxes. Slots with no interface stay
/// blank.
struct Grid {
    master: Rect,
    area: Rect,
    n: usize,
    cols: usize,
    /// Rows of boxes on a page. Zero if the terminal is too short for any.
    rows: usize,
    box_width: u16,
    box_height: u16,
}

impl Grid {
    fn new(body: Rect, n: usize) -> Self {
        let wanted = (body.height * 2 / 5)
            .max(MIN_MASTER_HEIGHT)
            .min(body.height);
        let space = body.height - wanted;
        let cols = ((body.width / MIN_BOX_WIDTH) as usize)
            .clamp(1, MAX_COLUMNS)
            .min(n.max(1));
        // Too short for a usable box: show none, rather than slivers.
        let rows = if space < 5 || n == 0 {
            0
        } else {
            ((space / MIN_BOX_HEIGHT) as usize).clamp(1, n.div_ceil(cols))
        };
        let box_height = if rows == 0 { 0 } else { space / rows as u16 };
        // What equal rows leave over goes to the rollup, so the bottom row stays on the footer.
        let used = box_height * rows as u16;
        let master_height = body.height - used;
        Grid {
            master: Rect {
                height: master_height,
                ..body
            },
            area: Rect {
                y: body.y + master_height,
                height: used,
                ..body
            },
            n,
            cols,
            rows,
            box_width: body.width / cols as u16,
            box_height,
        }
    }

    fn per_page(&self) -> usize {
        self.rows * self.cols
    }

    /// Always at least one, so there is always a current page.
    fn pages(&self) -> usize {
        match self.per_page() {
            0 => 1,
            per_page => self.n.div_ceil(per_page).max(1),
        }
    }

    fn first_index(&self, page: usize) -> usize {
        page * self.per_page()
    }

    /// Rects for a page's boxes, in interface order. Spare columns are shared out as gaps,
    /// so the outer boxes line up with the rollup's edges.
    fn rects(&self, page: usize) -> Vec<Rect> {
        let count = self
            .per_page()
            .min(self.n.saturating_sub(self.first_index(page)));
        let spare = self.area.width - self.box_width * self.cols as u16;
        (0..count)
            .map(|i| {
                let (row, col) = ((i / self.cols) as u16, (i % self.cols) as u16);
                let gaps = (self.cols as u16 - 1).max(1);
                Rect {
                    x: self.area.x + col * self.box_width + spare * col / gaps,
                    y: self.area.y + row * self.box_height,
                    width: self.box_width,
                    height: self.box_height,
                }
            })
            .collect()
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
    if grid.pages() > 1 {
        let first = grid.first_index(app.page);
        let shown = format!(
            "page {}/{} ({}-{} of {})",
            app.page + 1,
            grid.pages(),
            first + 1,
            first + grid.rects(app.page).len(),
            grid.n
        );
        key(buf, "←→", &shown);
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
        let used = detail_row_count(inner.height);
        draw_details(buf, inner, app, d, used);
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

/// One value on a detail row.
struct Seg {
    label: &'static str,
    value: String,
    color: Option<Color>,
    /// Long values may be cut short to fit. Others are all or nothing.
    elastic: bool,
}

impl Seg {
    fn new(value: impl Into<String>) -> Self {
        Seg {
            label: "",
            value: value.into(),
            color: None,
            elastic: false,
        }
    }

    fn label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
    }

    fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    fn elastic(mut self) -> Self {
        self.elastic = true;
        self
    }

    fn label_width(&self) -> usize {
        if self.label.is_empty() {
            0
        } else {
            self.label.len() + 1
        }
    }
}

/// The detail rows, as `ip a` lays them out, in the order they're displayed. The number is
/// each row's rank for inclusion when a box has too little height for all of them.
const DETAIL_ROWS: [(&str, usize); 5] = [
    ("type", 0),
    ("link", 2),
    ("inet", 1),
    ("inet6", 3),
    ("ether", 4),
];

/// How many detail rows a box shows. Every box is the same size, so they all show the same
/// rows on the same lines.
fn detail_row_count(inner_height: u16) -> u16 {
    let spare = inner_height.saturating_sub(GRAPH_MIN_ROWS);
    if spare > 0 {
        spare.min(DETAIL_ROWS.len() as u16)
    } else {
        u16::from(inner_height >= 4)
    }
}

fn type_segments(d: &Details, app: &App) -> Vec<Seg> {
    let mut row = vec![Seg::new(d.kind.clone()).color(app.theme.hi)];
    if let Some(h) = &d.hardware {
        row.push(Seg::new(h.clone()).elastic());
    }
    if let Some(v) = &d.driver {
        row.push(Seg::new(v.clone()).label("driver"));
    }
    row
}

fn link_segments(d: &Details, app: &App) -> Vec<Seg> {
    let t = &app.theme;
    let mut row = Vec::new();
    if d.kind == "Wi-Fi" {
        let Some(w) = &d.wifi else {
            return vec![Seg::new("link info unavailable").color(t.inactive)];
        };
        row.push(match &w.ssid {
            Some(ssid) => Seg::new(ssid.clone())
                .label("SSID")
                .color(t.title)
                .elastic(),
            None if w.hidden => Seg::new("hidden SSID").color(t.inactive),
            None if w.signal_dbm.is_none() => Seg::new("not connected").color(t.inactive),
            None => Seg::new("scan unavailable").color(t.inactive),
        });
        if let Some(s) = w.signal_dbm {
            let c = if s >= -60 {
                t.good
            } else if s <= -75 {
                t.bad
            } else {
                t.fg
            };
            row.push(Seg::new(format!("{s} dBm")).color(c));
        }
        row.extend(w.band().map(Seg::new));
        row.extend(w.generation.map(Seg::new));
        row.extend(w.tx_mbps.map(|r| Seg::new(format!("{r:.0} Mbps"))));
    } else if d.kind.starts_with("ZeroTier") {
        match &d.zerotier {
            Some(Info::Network(n)) => {
                if let Some(name) = &n.name {
                    row.push(Seg::new(name.clone()).color(t.title).elastic());
                }
                row.push(Seg::new(n.id.clone()));
                if let Some(status) = &n.status {
                    let c = if status == "OK" { t.good } else { t.bad };
                    row.push(Seg::new(status.clone()).color(c));
                }
                row.extend(n.kind.clone().map(Seg::new));
            }
            Some(Info::Unavailable(why)) => row.push(Seg::new(*why).color(t.inactive)),
            None => {}
        }
    } else if d.kind == "Ethernet" {
        row.extend(d.speed.map(|s| Seg::new(fmt_speed(s))));
        row.extend(d.duplex.as_ref().map(|v| Seg::new(format!("{v} duplex"))));
    }
    row
}

/// Columns taken by `+N` and the gap before it. `N` can be wider than one digit.
fn more_span(remaining: usize) -> usize {
    const GAP: usize = 2;
    let mut digits = 1;
    let mut n = remaining;
    while n >= 10 {
        digits += 1;
        n /= 10;
    }
    1 + digits + GAP
}

/// As many addresses as fit in `width`, then a count of the rest.
fn address_segments(addresses: &[String], width: usize) -> Vec<Seg> {
    const GAP: usize = 2;
    let mut row = Vec::new();
    let mut used = 0;
    for (i, a) in addresses.iter().enumerate() {
        let len = a.chars().count() + if i == 0 { 0 } else { GAP };
        let remaining = addresses.len() - i - 1;
        let reserve = if remaining == 0 {
            0
        } else {
            more_span(remaining)
        };
        if used + len + reserve > width {
            if i == 0 {
                row.push(Seg::new(a.clone()).elastic());
                break;
            }
            row.push(Seg::new(format!("+{}", addresses.len() - i)));
            break;
        }
        row.push(Seg::new(a.clone()));
        used += len;
    }
    row
}

fn ether_segments(d: &Details) -> Vec<Seg> {
    let mut row: Vec<Seg> = d.mac.iter().map(|m| Seg::new(m.clone())).collect();
    row.extend(d.mtu.map(|v| Seg::new(v.to_string()).label("mtu")));
    row
}

fn draw_details(buf: &mut Buffer, r: Rect, app: &App, d: &Details, rows: u16) {
    let t = &app.theme;
    let value_width = (r.width.saturating_sub(DETAIL_LABEL_WIDTH)) as usize;
    let mut y = r.y;
    for (label, rank) in DETAIL_ROWS {
        if rank >= rows as usize {
            continue;
        }
        let segments = match label {
            "type" => type_segments(d, app),
            "link" => link_segments(d, app),
            "inet" => address_segments(&d.ipv4, value_width),
            "inet6" => address_segments(&d.ipv6, value_width),
            _ => ether_segments(d),
        };
        let line = Rect { y, height: 1, ..r };
        put(buf, r.x, y, line, label, Style::new().fg(t.inactive));
        draw_segments(buf, line, app, &segments);
        y += 1;
    }
}

/// Lays values out along one row and never wraps: what doesn't fit is cut or left off.
fn draw_segments(buf: &mut Buffer, line: Rect, app: &App, segments: &[Seg]) {
    const GAP: u16 = 2;
    const MIN_ELASTIC: usize = 6;
    let t = &app.theme;
    let mut x = line.x + DETAIL_LABEL_WIDTH;
    if x >= line.right() {
        return;
    }
    if segments.is_empty() {
        put(buf, x, line.y, line, "—", Style::new().fg(t.inactive));
        return;
    }
    for seg in segments {
        let room = line.right().saturating_sub(x) as usize;
        let label = seg.label_width();
        let len = label + seg.value.chars().count();
        let value = if len <= room {
            seg.value.clone()
        } else if seg.elastic && room >= label + MIN_ELASTIC {
            truncate(&seg.value, room - label)
        } else {
            return;
        };
        if label > 0 {
            x = put(buf, x, line.y, line, seg.label, Style::new().fg(t.inactive)) + 1;
        }
        let style = Style::new().fg(seg.color.unwrap_or(t.fg));
        x = put(buf, x, line.y, line, &value, style) + GAP;
    }
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
    fn grid_never_leaves_the_body_and_all_boxes_are_equal() {
        for w in [10u16, 40, 59, 60, 80, 120, 131, 179, 180, 200, 400] {
            for h in [1u16, 5, 8, 14, 24, 50, 100] {
                for n in [0usize, 1, 2, 3, 4, 5, 7, 40] {
                    let body = Rect {
                        x: 0,
                        y: 0,
                        width: w,
                        height: h,
                    };
                    let g = Grid::new(body, n);
                    assert!(g.master.bottom() <= h);
                    assert!(g.cols <= MAX_COLUMNS);
                    let mut seen = 0;
                    for page in 0..g.pages() {
                        for r in g.rects(page) {
                            assert!(r.right() <= w && r.bottom() <= h, "{w}x{h} n={n} {r:?}");
                            assert!(r.y >= g.master.bottom(), "{w}x{h} n={n} {r:?}");
                            assert_eq!((r.width, r.height), (g.box_width, g.box_height));
                            assert!(r.width > 0 && r.height > 0);
                            seen += 1;
                        }
                    }
                    if g.rows > 0 {
                        assert_eq!(seen, n, "{w}x{h}: every interface is on some page");
                    }
                }
            }
        }
    }

    fn grid(w: u16, h: u16, n: usize) -> Grid {
        Grid::new(
            Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            },
            n,
        )
    }

    #[test]
    fn columns_are_at_most_three_and_at_least_one() {
        assert_eq!(grid(400, 60, 7).cols, 3);
        assert_eq!(grid(180, 60, 7).cols, 3);
        assert_eq!(grid(179, 60, 7).cols, 2);
        assert_eq!(grid(120, 60, 7).cols, 2);
        assert_eq!(grid(40, 60, 7).cols, 1);
        assert_eq!(grid(400, 60, 2).cols, 2);
    }

    #[test]
    fn leftover_slots_stay_blank_and_boxes_keep_their_size() {
        let g = grid(200, 60, 4);
        let rects = g.rects(0);
        assert_eq!(rects.len(), 4);
        assert_eq!(rects[3].width, rects[0].width);
        assert_eq!(rects[3].x, 0, "fourth box starts a new row at the left");
        assert_eq!(rects[3].y, rects[0].y + rects[0].height);
    }

    #[test]
    fn outer_boxes_line_up_with_the_rollup() {
        for w in [180u16, 181, 182, 200, 241] {
            let g = grid(w, 60, 3);
            let rects = g.rects(0);
            assert_eq!(rects[0].x, 0);
            assert_eq!(rects[2].right(), w, "width {w}");
        }
    }

    #[test]
    fn bottom_row_sits_on_the_footer() {
        for h in 20..60u16 {
            let g = grid(120, h, 5);
            assert_eq!(g.area.bottom(), h);
            assert_eq!(g.master.bottom(), g.area.y);
        }
    }

    #[test]
    fn pages_divide_interfaces_and_the_last_may_be_short() {
        let g = grid(120, 30, 5); // two columns, one row
        assert_eq!((g.cols, g.rows), (2, 1));
        assert_eq!(g.pages(), 3);
        assert_eq!(g.rects(0).len(), 2);
        assert_eq!(g.rects(2).len(), 1);
        assert_eq!(g.rects(2)[0].width, g.rects(0)[0].width);
        assert_eq!(g.first_index(2), 4);
    }

    #[test]
    fn page_is_clamped_and_shown_in_the_footer() {
        let mut m = crate::net::Monitor::default();
        for name in ["a", "b", "c"] {
            let mut i = wifi_iface();
            i.name = name.into();
            m.ifaces.push(i);
        }
        let mut app = test_app(m);
        app.page = usize::MAX;
        let text = render(&mut app, 120, 30).join("\n");
        assert_eq!(app.page, 1);
        assert!(text.contains("page 2/2 (3-3 of 3)"), "{text}");
        assert!(!text.contains("┐a┌") && text.contains("┐c┌"), "{text}");
    }

    #[test]
    fn detail_rows_line_up_across_boxes_whatever_they_contain() {
        // Very different contents, at many sizes.
        let mut m = crate::net::Monitor::default();
        m.ifaces.push(wifi_iface());
        let mut other = wifi_iface();
        other.name = "lo".into();
        other.details = Some(Details {
            kind: "Loopback".into(),
            mtu: Some(65536),
            ipv4: vec!["127.0.0.1/8".into()],
            ..Default::default()
        });
        m.ifaces.push(other);
        let mut app = test_app(m);
        for w in (120..=178).step_by(7) {
            for h in [24u16, 30, 37, 44, 50] {
                let lines = render(&mut app, w, h);
                let top = lines.iter().position(|l| l.contains("┐wlan0┌")).unwrap();
                let half = w as usize / 2;
                let col = |l: &str, from: usize, to: usize| -> String {
                    l.chars().skip(from).take(to - from).take(7).collect()
                };
                for line in &lines[top + 1..] {
                    let left = col(line, 1, half);
                    let start = line
                        .chars()
                        .enumerate()
                        .skip(half)
                        .find(|(_, c)| *c == '│' || *c == '╰');
                    let Some((start, _)) = start else { continue };
                    let right = col(line, start + 1, start + 8);
                    for label in ["type", "link", "inet6", "inet ", "ether"] {
                        assert_eq!(
                            left.starts_with(label),
                            right.starts_with(label),
                            "{w}x{h}: {label:?} misaligned:\n{}",
                            lines.join("\n")
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn missing_details_show_a_dash_rather_than_closing_up() {
        let mut m = crate::net::Monitor::default();
        let mut i = wifi_iface();
        i.details = Some(Details {
            kind: "Bridge".into(),
            ..Default::default()
        });
        m.ifaces.push(i);
        let text = render(&mut test_app(m), 100, 40).join("\n");
        for label in ["link", "inet ", "inet6", "ether"] {
            assert!(text.contains(&format!("{label:<6}—")), "{label}:\n{text}");
        }
    }

    #[test]
    fn long_values_never_wrap_or_leave_their_row() {
        let mut m = crate::net::Monitor::default();
        let mut i = wifi_iface();
        let d = i.details.as_mut().unwrap();
        d.hardware = Some("An Extremely Long Hardware Name ".repeat(5));
        d.ipv6 = (0..6).map(|n| format!("2001:db8::{n}/64")).collect();
        m.ifaces.push(i);
        let lines = render(&mut test_app(m), 62, 40);
        let text = lines.join("\n");
        assert!(text.contains("…"), "hardware is cut:\n{text}");
        assert!(text.contains("+"), "extra addresses are counted:\n{text}");
        assert_eq!(text.matches("type  ").count(), 1);
        assert_eq!(text.matches("inet6 ").count(), 1);
        assert!(lines.iter().all(|l| l.chars().count() == 62));
    }

    #[test]
    fn address_list_counts_what_does_not_fit() {
        let a: Vec<String> = ["aaaaaaaaaa", "bbbbbbbbbb", "cccccccccc"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let values = |width| -> Vec<String> {
            address_segments(&a, width)
                .into_iter()
                .map(|s| s.value)
                .collect()
        };
        assert_eq!(values(34), ["aaaaaaaaaa", "bbbbbbbbbb", "cccccccccc"]);
        assert_eq!(values(30), ["aaaaaaaaaa", "bbbbbbbbbb", "+1"]);
        assert_eq!(values(20), ["aaaaaaaaaa", "+2"]);
        assert_eq!(values(5), ["aaaaaaaaaa"]);
        assert!(address_segments(&[], 30).is_empty());

        // "+10" is three columns. Reserving only "+9" would draw past `width`.
        let many = vec!["x".to_string(); 11];
        let shown = |width: usize| -> Vec<String> {
            address_segments(&many, width)
                .into_iter()
                .map(|s| s.value)
                .collect()
        };
        assert_eq!(shown(5), ["x"]);
        assert_eq!(shown(6), ["x", "+10"]);
    }

    #[test]
    fn detail_rows_grow_with_the_box_and_keep_the_graph_usable() {
        assert_eq!(detail_row_count(3), 0);
        assert_eq!(detail_row_count(4), 1);
        assert_eq!(detail_row_count(6), 1);
        assert_eq!(detail_row_count(8), 2);
        assert_eq!(detail_row_count(10), 4);
        assert_eq!(detail_row_count(40), 5);
    }

    #[test]
    fn zerotier_box_shows_network_or_says_why_not() {
        let box_with = |zt: Option<Info>| {
            let mut m = crate::net::Monitor::default();
            let mut i = wifi_iface();
            i.details = Some(Details {
                kind: "ZeroTier VPN (TAP)".into(),
                zerotier: zt,
                ..Default::default()
            });
            m.ifaces.push(i);
            render(&mut test_app(m), 100, 40).join("\n")
        };
        let net = crate::zerotier::Network {
            id: "8056c2e21c000001".into(),
            name: Some("office".into()),
            status: Some("OK".into()),
            kind: Some("PRIVATE".into()),
            ..Default::default()
        };
        let text = box_with(Some(Info::Network(net)));
        assert!(
            text.contains("link  office  8056c2e21c000001  OK  PRIVATE"),
            "{text}"
        );
        let text = box_with(Some(Info::Unavailable("no access to zerotier-cli")));
        assert!(text.contains("link  no access to zerotier-cli"), "{text}");
        let text = box_with(None);
        assert!(text.contains("link  —"), "{text}");
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
            details: Some(Details {
                kind: "Wi-Fi".into(),
                hardware: Some("Intel Wi-Fi 6E AX211 160MHz".into()),
                driver: Some("iwlwifi".into()),
                mac: Some("02:00:00:00:00:01".into()),
                mtu: Some(1500),
                ipv4: vec!["192.0.2.1/24".into()],
                ipv6: vec!["2001:db8::1/64".into(), "fe80::1/64".into()],
                wifi: Some(crate::wifi::WifiLink {
                    ssid: Some("homenet".into()),
                    freq_mhz: Some(5220),
                    width_mhz: Some(80),
                    signal_dbm: Some(-59),
                    tx_mbps: Some(866.0),
                    generation: Some("Wi-Fi 6"),
                    ..Default::default()
                }),
                ..Default::default()
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
            "ether 02:00:00:00:00:01  mtu 1500",
            "inet  192.0.2.1/24",
            "inet6 2001:db8::1/64  fe80::1/64",
        ] {
            assert!(text.contains(want), "missing {want:?} in:\n{text}");
        }
    }

    #[test]
    fn wifi_link_row_distinguishes_hidden_from_a_failed_scan() {
        let row = |link: crate::wifi::WifiLink| {
            let mut m = crate::net::Monitor::default();
            let mut i = wifi_iface();
            i.details.as_mut().unwrap().wifi = Some(link);
            m.ifaces.push(i);
            render(&mut test_app(m), 100, 40).join("\n")
        };
        let hidden = row(crate::wifi::WifiLink {
            hidden: true,
            signal_dbm: Some(-70),
            ..Default::default()
        });
        assert!(hidden.contains("hidden SSID"), "{hidden}");
        assert!(!hidden.contains("scan unavailable"));
        let failed = row(crate::wifi::WifiLink {
            signal_dbm: Some(-70),
            ..Default::default()
        });
        assert!(failed.contains("scan unavailable"), "{failed}");
        assert!(!failed.contains("hidden SSID"));
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
        app.page = usize::MAX;
        for w in [1u16, 9, 10, 12, 13, 30, 42, 44, 60, 80, 200] {
            for h in [1u16, 2, 3, 4, 5, 8, 12, 24, 60] {
                let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                term.draw(|f| draw(f, &mut app)).unwrap();
            }
        }
    }
}
