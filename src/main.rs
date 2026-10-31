//! nitop: a btop-style terminal monitor for network interfaces.

#[cfg(not(target_os = "linux"))]
compile_error!("nitop only supports Linux: it reads /proc, /sys and nl80211.");

mod ifinfo;
mod meter;
mod net;
#[cfg(test)]
mod screenshot;
mod theme;
mod ui;
mod wifi;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::io::IsTerminal;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const INTERVALS_MS: [u64; 7] = [100, 250, 500, 1000, 2000, 5000, 10000];
const DEFAULT_INTERVAL: usize = 3;
/// Level meter animation rate. Nothing redraws for it once the meter has settled.
const FRAME: Duration = Duration::from_millis(50);

const HELP: &str = "\
nitop: a btop-style terminal monitor for network interfaces

Usage: nitop [OPTIONS]

Options:
  -h, --help     Print this help
  -V, --version  Print the version

Keys:
  q, Esc, Ctrl-C        Quit
  b                     Show rates in bytes or bits
  + / -                 Slower / faster sampling (100 ms to 10 s)
  Up/Down, j/k          Scroll interface boxes
  PgUp/PgDn, Home/End   Scroll further
";

pub struct App {
    pub monitor: net::Monitor,
    pub meters: meter::Meters,
    pub theme: theme::Theme,
    pub bits: bool,
    pub interval: usize,
    /// First visible row of interface boxes.
    pub scroll: usize,
}

impl App {
    pub fn new(monitor: net::Monitor, theme: theme::Theme) -> Self {
        App {
            monitor,
            meters: meter::Meters::default(),
            theme,
            bits: false,
            interval: DEFAULT_INTERVAL,
            scroll: 0,
        }
    }

    pub fn interval_ms(&self) -> u64 {
        INTERVALS_MS[self.interval]
    }

    /// Applies a key press. `scroll` may overshoot here: drawing clamps it to what fits.
    fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers) -> KeyResult {
        if mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
            return KeyResult::Quit;
        }
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return KeyResult::Quit,
            KeyCode::Char('b') => self.bits = !self.bits,
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.interval = (self.interval + 1).min(INTERVALS_MS.len() - 1)
            }
            KeyCode::Char('-') => self.interval = self.interval.saturating_sub(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(5),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(5),
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = usize::MAX,
            _ => return KeyResult::Ignored,
        }
        KeyResult::Changed
    }
}

#[derive(Debug, PartialEq)]
enum KeyResult {
    Quit,
    Changed,
    Ignored,
}

#[derive(Debug, PartialEq)]
enum Action {
    Run,
    Print(String),
    BadUsage(String),
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Action {
    match args.into_iter().next().as_deref() {
        None => Action::Run,
        Some("-h" | "--help") => Action::Print(HELP.to_string()),
        Some("-V" | "--version") => Action::Print(format!("nitop {}\n", env!("CARGO_PKG_VERSION"))),
        Some(other) => Action::BadUsage(format!(
            "nitop: unknown argument '{other}'\nTry 'nitop --help'."
        )),
    }
}

fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)) {
        Action::Print(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Action::BadUsage(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
        Action::Run => match run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("nitop: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

fn run() -> std::io::Result<()> {
    if !std::io::stdout().is_terminal() {
        return Err(std::io::Error::other("must be run in a terminal"));
    }
    // Before touching the terminal, so a failure prints normally.
    let (monitor, mut last) = net::Monitor::new()?;
    let mut app = App::new(monitor, theme::Theme::load());
    let mut terminal = ratatui::try_init()?;
    let result = event_loop(&mut terminal, &mut app, &mut last);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    last: &mut Instant,
) -> std::io::Result<()> {
    let mut dirty = true;
    let mut last_frame = Instant::now();
    loop {
        if dirty {
            terminal.draw(|f| ui::draw(f, app))?;
            dirty = false;
        }
        let period = Duration::from_millis(app.interval_ms());
        let wait = period
            .saturating_sub(last.elapsed())
            .min(FRAME.saturating_sub(last_frame.elapsed()));
        if event::poll(wait)? {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    match app.handle_key(k.code, k.modifiers) {
                        KeyResult::Quit => return Ok(()),
                        KeyResult::Changed => dirty = true,
                        KeyResult::Ignored => {}
                    }
                }
                Event::Resize(..) => dirty = true,
                _ => {}
            }
        }
        if last.elapsed() >= period {
            let now = Instant::now();
            let dt = now.duration_since(*last).as_secs_f64();
            *last = now;
            // A transient read failure skips one tick; the next delta covers the gap
            // in bytes, and rates stay correct from then on.
            if app.monitor.sample(dt).is_ok() {
                app.meters.rx.feed(app.monitor.master.rx.rate);
                app.meters.tx.feed(app.monitor.master.tx.rate);
                dirty = true;
            }
        }
        if last_frame.elapsed() >= FRAME {
            dirty |= app.meters.tick(last_frame.elapsed().as_secs_f64());
            last_frame = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(net::Monitor::default(), theme::Theme::from_str(""))
    }

    fn press(app: &mut App, code: KeyCode) -> KeyResult {
        app.handle_key(code, KeyModifiers::NONE)
    }

    #[test]
    fn args_are_parsed_and_unknown_ones_rejected() {
        let parse = |a: &[&str]| parse_args(a.iter().map(|s| s.to_string()));
        assert_eq!(parse(&[]), Action::Run);
        assert_eq!(parse(&["--help"]), Action::Print(HELP.into()));
        assert_eq!(parse(&["-h"]), Action::Print(HELP.into()));
        assert!(matches!(parse(&["-V"]), Action::Print(v) if v.starts_with("nitop ")));
        assert!(matches!(parse(&["--nope"]), Action::BadUsage(m) if m.contains("--nope")));
    }

    #[test]
    fn quit_keys_quit() {
        assert_eq!(press(&mut app(), KeyCode::Char('q')), KeyResult::Quit);
        assert_eq!(press(&mut app(), KeyCode::Esc), KeyResult::Quit);
        let ctrl_c = app().handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(ctrl_c, KeyResult::Quit);
        assert_eq!(press(&mut app(), KeyCode::Char('c')), KeyResult::Ignored);
    }

    #[test]
    fn b_toggles_units() {
        let mut a = app();
        press(&mut a, KeyCode::Char('b'));
        assert!(a.bits);
        press(&mut a, KeyCode::Char('b'));
        assert!(!a.bits);
    }

    #[test]
    fn interval_stops_at_both_ends() {
        let mut a = app();
        assert_eq!(a.interval_ms(), 1000);
        for _ in 0..20 {
            press(&mut a, KeyCode::Char('+'));
        }
        assert_eq!(a.interval_ms(), 10000);
        for _ in 0..20 {
            press(&mut a, KeyCode::Char('-'));
        }
        assert_eq!(a.interval_ms(), 100);
    }

    #[test]
    fn scrolling_cannot_underflow_or_overflow() {
        let mut a = app();
        press(&mut a, KeyCode::Up);
        press(&mut a, KeyCode::PageUp);
        assert_eq!(a.scroll, 0);
        press(&mut a, KeyCode::End);
        press(&mut a, KeyCode::Down);
        press(&mut a, KeyCode::PageDown);
        assert_eq!(a.scroll, usize::MAX);
        press(&mut a, KeyCode::Home);
        assert_eq!(a.scroll, 0);
    }
}
