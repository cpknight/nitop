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
mod zerotier;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::atomic::{AtomicI32, Ordering};
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
  q, Esc, Ctrl-C               Quit
  b                            Show rates in bytes or bits
  + / -                        Slower / faster sampling (100 ms to 10 s)
  Arrows, j/k, h/l, PgUp/PgDn  Previous / next page of interfaces
  Home/End                     First / last page
";

pub struct App {
    pub monitor: net::Monitor,
    pub meters: meter::Meters,
    pub theme: theme::Theme,
    pub bits: bool,
    pub interval: usize,
    /// Current page of interface boxes.
    pub page: usize,
}

impl App {
    pub fn new(monitor: net::Monitor, theme: theme::Theme) -> Self {
        App {
            monitor,
            meters: meter::Meters::default(),
            theme,
            bits: false,
            interval: DEFAULT_INTERVAL,
            page: 0,
        }
    }

    pub fn interval_ms(&self) -> u64 {
        INTERVALS_MS[self.interval]
    }

    /// Applies a key press. `page` may overshoot here: drawing clamps it to what fits.
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
            KeyCode::Up
            | KeyCode::Left
            | KeyCode::PageUp
            | KeyCode::Char('k')
            | KeyCode::Char('h') => self.page = self.page.saturating_sub(1),
            KeyCode::Down
            | KeyCode::Right
            | KeyCode::PageDown
            | KeyCode::Char('j')
            | KeyCode::Char('l') => self.page = self.page.saturating_add(1),
            KeyCode::Home => self.page = 0,
            KeyCode::End => self.page = usize::MAX,
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
    let (monitor, started) = net::Monitor::new()?;
    let mut app = App::new(monitor, theme::Theme::load());
    let mut clock = SampleClock::new(started);
    // try_init enables raw mode before it can fail. Restore on that path too,
    // or the shell stays raw when the alternate screen or terminal setup fails.
    let mut terminal = ratatui::try_init().inspect_err(|_| ratatui::restore())?;
    install_signal_handlers();
    let result = event_loop(&mut terminal, &mut app, &mut clock);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    clock: &mut SampleClock,
) -> std::io::Result<()> {
    let mut dirty = true;
    let mut last_frame = Instant::now();
    loop {
        match take_signal(terminal)? {
            SignalAction::Quit => return Ok(()),
            SignalAction::Redraw => dirty = true,
            SignalAction::None => {}
        }
        if dirty {
            terminal.draw(|f| ui::draw(f, app))?;
            dirty = false;
        }
        let period = Duration::from_millis(app.interval_ms());
        let now = Instant::now();
        let wait = clock
            .wait(now, period)
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
        match take_signal(terminal)? {
            SignalAction::Quit => return Ok(()),
            SignalAction::Redraw => dirty = true,
            SignalAction::None => {}
        }
        let now = Instant::now();
        if clock.due(now, period) {
            let dt = clock.gap(now);
            // Leave the clock where it is on failure. Advancing it would divide
            // the bytes of two intervals by one interval and stick that spike as the peak.
            if app.monitor.sample(dt).is_ok() {
                clock.succeeded(now);
                app.meters.rx.feed(app.monitor.master.rx.rate);
                app.meters.tx.feed(app.monitor.master.tx.rate);
                dirty = true;
            } else {
                clock.failed(now, period);
            }
        }
        if last_frame.elapsed() >= FRAME {
            dirty |= app.meters.tick(last_frame.elapsed().as_secs_f64());
            last_frame = Instant::now();
        }
    }
}

/// Sample timing. A failed read does not move `last_ok`, so the next success
/// covers the whole gap, and `retry_after` keeps a persistent error from spinning.
struct SampleClock {
    last_ok: Instant,
    retry_after: Option<Instant>,
}

impl SampleClock {
    fn new(now: Instant) -> Self {
        Self {
            last_ok: now,
            retry_after: None,
        }
    }

    fn wait(&self, now: Instant, period: Duration) -> Duration {
        match self.retry_after {
            Some(at) if now < at => at.saturating_duration_since(now),
            _ => period.saturating_sub(now.saturating_duration_since(self.last_ok)),
        }
    }

    fn due(&self, now: Instant, period: Duration) -> bool {
        if self.retry_after.is_some_and(|at| now < at) {
            return false;
        }
        now.saturating_duration_since(self.last_ok) >= period
    }

    fn gap(&self, now: Instant) -> f64 {
        now.saturating_duration_since(self.last_ok).as_secs_f64()
    }

    fn succeeded(&mut self, now: Instant) {
        self.last_ok = now;
        self.retry_after = None;
    }

    fn failed(&mut self, now: Instant, period: Duration) {
        self.retry_after = Some(now + period);
    }
}

static SIGNAL: AtomicI32 = AtomicI32::new(0);

enum SignalAction {
    None,
    Quit,
    Redraw,
}

extern "C" fn on_signal(sig: libc::c_int) {
    // A fatal signal replaces a pending suspend. Suspend must not hide a request to exit.
    if sig == libc::SIGTSTP {
        let _ = SIGNAL.compare_exchange(0, sig, Ordering::Relaxed, Ordering::Relaxed);
    } else {
        SIGNAL.store(sig, Ordering::Relaxed);
    }
}

fn install_signal_handlers() {
    for sig in [libc::SIGTERM, libc::SIGHUP, libc::SIGTSTP] {
        install_one(sig);
    }
}

fn install_one(sig: libc::c_int) {
    // SAFETY: `action` is a valid sigaction, and the handler only stores an atomic.
    // SA_RESTART is left unset so a blocked poll returns and the loop can restore the terminal.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_signal as libc::sighandler_t;
        libc::sigemptyset(std::ptr::addr_of_mut!(action.sa_mask));
        libc::sigaction(sig, &action, std::ptr::null_mut());
    }
}

fn take_signal(terminal: &mut ratatui::DefaultTerminal) -> std::io::Result<SignalAction> {
    match SIGNAL.swap(0, Ordering::SeqCst) {
        0 => Ok(SignalAction::None),
        sig if sig == libc::SIGTSTP => {
            suspend(terminal)?;
            Ok(SignalAction::Redraw)
        }
        _ => Ok(SignalAction::Quit),
    }
}

fn suspend(terminal: &mut ratatui::DefaultTerminal) -> std::io::Result<()> {
    ratatui::restore();
    // SAFETY: the default action stops the process. The handler is put back after
    // resume so a later Ctrl-Z still restores the terminal before stopping.
    unsafe {
        libc::signal(libc::SIGTSTP, libc::SIG_DFL);
        libc::raise(libc::SIGTSTP);
        install_one(libc::SIGTSTP);
    }
    ratatui::crossterm::terminal::enable_raw_mode()?;
    ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::terminal::EnterAlternateScreen
    )?;
    terminal.clear()?;
    Ok(())
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
    fn paging_cannot_underflow_or_overflow() {
        let mut a = app();
        press(&mut a, KeyCode::Up);
        press(&mut a, KeyCode::PageUp);
        assert_eq!(a.page, 0);
        for key in [
            KeyCode::Down,
            KeyCode::Right,
            KeyCode::PageDown,
            KeyCode::Char('j'),
        ] {
            press(&mut a, key);
        }
        assert_eq!(a.page, 4);
        press(&mut a, KeyCode::Left);
        assert_eq!(a.page, 3);
        press(&mut a, KeyCode::End);
        press(&mut a, KeyCode::Down);
        assert_eq!(a.page, usize::MAX);
        press(&mut a, KeyCode::Home);
        assert_eq!(a.page, 0);
    }

    #[test]
    fn failed_sample_keeps_the_clock_so_the_next_dt_covers_the_gap() {
        let t0 = Instant::now();
        let mut clock = SampleClock::new(t0);
        let period = Duration::from_secs(1);
        let t1 = t0 + period;
        assert!(clock.due(t1, period));
        assert!((clock.gap(t1) - 1.0).abs() < 0.05);
        clock.failed(t1, period);
        assert!(!clock.due(t1 + Duration::from_millis(100), period));
        let t2 = t1 + period;
        assert!(clock.due(t2, period));
        assert!((clock.gap(t2) - 2.0).abs() < 0.05);
        clock.succeeded(t2);
        assert!(!clock.due(t2, period));
        assert!(clock.gap(t2) < 0.05);
    }
}
