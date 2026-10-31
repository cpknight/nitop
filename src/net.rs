//! Interface statistics: reads kernel counters and derives rates and totals.

use crate::ifinfo::{self, Details};
use crate::wifi::Poller as WifiPoller;
use crate::zerotier::Poller;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::time::Instant;

/// Samples kept per series. Each terminal column shows two, so this covers 1024 columns.
const HISTORY: usize = 2048;
const IFF_UP: u32 = 0x1;
const IFF_LOOPBACK: u32 = 0x8;
/// Rereading addresses and link details every tick is wasteful; they rarely change.
const ADDR_REFRESH_SECS: f64 = 5.0;

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Counters {
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub rx_errs: u64,
    pub rx_drop: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
    pub tx_errs: u64,
    pub tx_drop: u64,
}

impl Counters {
    /// Field-wise `self - prev`. A counter that went backwards (interface recreated,
    /// driver reset) yields 0 rather than a huge bogus spike.
    fn delta(&self, prev: &Counters) -> Counters {
        Counters {
            rx_bytes: self.rx_bytes.saturating_sub(prev.rx_bytes),
            rx_packets: self.rx_packets.saturating_sub(prev.rx_packets),
            rx_errs: self.rx_errs.saturating_sub(prev.rx_errs),
            rx_drop: self.rx_drop.saturating_sub(prev.rx_drop),
            tx_bytes: self.tx_bytes.saturating_sub(prev.tx_bytes),
            tx_packets: self.tx_packets.saturating_sub(prev.tx_packets),
            tx_errs: self.tx_errs.saturating_sub(prev.tx_errs),
            tx_drop: self.tx_drop.saturating_sub(prev.tx_drop),
        }
    }

    fn add(&mut self, o: &Counters) {
        self.rx_bytes += o.rx_bytes;
        self.rx_packets += o.rx_packets;
        self.rx_errs += o.rx_errs;
        self.rx_drop += o.rx_drop;
        self.tx_bytes += o.tx_bytes;
        self.tx_packets += o.tx_packets;
        self.tx_errs += o.tx_errs;
        self.tx_drop += o.tx_drop;
    }
}

/// Parses `/proc/net/dev`. Malformed lines are skipped.
pub fn parse_proc_net_dev(text: &str) -> Vec<(String, Counters)> {
    text.lines()
        .filter_map(|line| {
            // The separator is the last colon: there may be no space after it.
            let (name, rest) = line.rsplit_once(':')?;
            let f: Vec<u64> = rest
                .split_whitespace()
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            if f.len() < 16 {
                return None;
            }
            Some((
                name.trim().to_string(),
                Counters {
                    rx_bytes: f[0],
                    rx_packets: f[1],
                    rx_errs: f[2],
                    rx_drop: f[3],
                    tx_bytes: f[8],
                    tx_packets: f[9],
                    tx_errs: f[10],
                    tx_drop: f[11],
                },
            ))
        })
        .collect()
}

/// One traffic direction.
#[derive(Default)]
pub struct Flow {
    /// Bytes per second, oldest first.
    pub history: VecDeque<f64>,
    pub rate: f64,
    pub peak: f64,
    pub packet_rate: f64,
    /// Bytes seen since this program started watching.
    pub session: u64,
}

impl Flow {
    fn push(&mut self, bytes: u64, packets: u64, dt: f64) {
        self.rate = bytes as f64 / dt;
        self.packet_rate = packets as f64 / dt;
        self.peak = self.peak.max(self.rate);
        self.session += bytes;
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(self.rate);
    }
}

/// Traffic history and statistics for one interface or for the rollup.
#[derive(Default)]
pub struct Series {
    pub rx: Flow,
    pub tx: Flow,
    /// Latest absolute kernel counters.
    pub counters: Counters,
    /// Seconds of observation, for averages.
    pub elapsed: f64,
}

impl Series {
    fn push(&mut self, delta: &Counters, current: Counters, dt: f64) {
        self.rx.push(delta.rx_bytes, delta.rx_packets, dt);
        self.tx.push(delta.tx_bytes, delta.tx_packets, dt);
        self.counters = current;
        self.elapsed += dt;
    }

    pub fn avg(&self, flow: &Flow) -> f64 {
        if self.elapsed > 0.0 {
            flow.session as f64 / self.elapsed
        } else {
            0.0
        }
    }
}

pub struct Iface {
    pub name: String,
    pub series: Series,
    pub loopback: bool,
    pub up: bool,
    /// `None` until first read, so a newly seen interface gets filled in right away.
    pub details: Option<Details>,
}

#[derive(Default)]
pub struct Monitor {
    pub ifaces: Vec<Iface>,
    /// Rollup of every non-loopback interface.
    pub master: Series,
    /// Seconds until addresses and link details are next reread; starts at 0 so the first sample reads them.
    addr_refresh_in: f64,
    /// Started when the first ZeroTier interface appears.
    zerotier: Option<Poller>,
    /// Started when the first Wi-Fi interface appears.
    wifi: Option<WifiPoller>,
}

impl Monitor {
    /// Takes the baseline sample. Fails if the kernel's counters are unavailable.
    pub fn new() -> std::io::Result<(Self, Instant)> {
        let mut m = Monitor::default();
        m.sample(0.0)?;
        Ok((m, Instant::now()))
    }

    /// Reads current counters and records one sample covering `dt` seconds.
    pub fn sample(&mut self, dt: f64) -> std::io::Result<()> {
        let text = fs::read_to_string("/proc/net/dev")?;
        self.apply(parse_proc_net_dev(&text), dt);
        for i in &mut self.ifaces {
            read_sysfs(i);
        }
        self.ifaces
            .sort_by(|a, b| (a.loopback, &a.name).cmp(&(b.loopback, &b.name)));

        self.addr_refresh_in -= dt;
        let new_iface = self.ifaces.iter().any(|i| i.details.is_none());
        if self.addr_refresh_in <= 0.0 || new_iface {
            self.addr_refresh_in = ADDR_REFRESH_SECS;
            let mut addrs = read_addrs();
            for i in &mut self.ifaces {
                let name = &i.name;
                let details = i.details.get_or_insert_with(|| ifinfo::read_static(name));
                ifinfo::refresh(name, details);
                let (v4, v6) = addrs.remove(name).unwrap_or_default();
                details.ipv4 = v4;
                details.ipv6 = v6;
            }
        }
        // Both of these are a lock and a copy. The work happens on other threads.
        self.apply_wifi();
        for i in &mut self.ifaces {
            if let Some(d) = i
                .details
                .as_mut()
                .filter(|d| d.kind.starts_with("ZeroTier"))
            {
                let poller = self.zerotier.get_or_insert_with(Poller::start);
                d.zerotier = poller.lookup(&i.name, d.mac.as_deref());
            }
        }
        Ok(())
    }

    fn apply_wifi(&mut self) {
        let found: Vec<(usize, u32)> = self
            .ifaces
            .iter()
            .enumerate()
            .filter(|(_, iface)| iface.details.as_ref().is_some_and(|d| d.kind == "Wi-Fi"))
            .filter_map(|(n, iface)| ifinfo::ifindex(&iface.name).map(|idx| (n, idx)))
            .collect();
        if found.is_empty() {
            return;
        }
        let poller = self.wifi.get_or_insert_with(WifiPoller::start);
        poller.watch(found.iter().map(|(_, idx)| *idx));
        for (n, idx) in found {
            if let Some(link) = poller.get(idx)
                && let Some(details) = self.ifaces[n].details.as_mut()
            {
                details.wifi = Some(link);
            }
        }
    }

    pub(crate) fn apply(&mut self, parsed: Vec<(String, Counters)>, dt: f64) {
        let mut old: HashMap<String, Iface> =
            self.ifaces.drain(..).map(|i| (i.name.clone(), i)).collect();
        let mut sum_delta = Counters::default();
        let mut sum_current = Counters::default();

        for (name, cur) in parsed {
            let loopback = name == "lo";
            let mut iface = old.remove(&name).unwrap_or_else(|| Iface {
                name,
                // Baseline an interface on first sight, so traffic from before it
                // appeared is not counted as this tick's traffic.
                series: Series {
                    counters: cur,
                    ..Default::default()
                },
                loopback,
                up: true,
                details: None,
            });
            let delta = cur.delta(&iface.series.counters);
            if dt > 0.0 {
                iface.series.push(&delta, cur, dt);
            }
            if !iface.loopback {
                sum_delta.add(&delta);
                sum_current.add(&cur);
            }
            self.ifaces.push(iface);
        }

        if dt > 0.0 {
            self.master.push(&sum_delta, sum_current, dt);
        } else {
            self.master.counters = sum_current;
        }
    }
}

fn read_sysfs(i: &mut Iface) {
    let dir = format!("/sys/class/net/{}", i.name);
    let read = |file: &str| fs::read_to_string(format!("{dir}/{file}")).ok();
    let flags = read("flags")
        .and_then(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
    let oper = read("operstate").unwrap_or_default();
    // Tunnels and loopback report operstate "unknown" while working normally.
    i.up = flags & IFF_UP != 0 && matches!(oper.trim(), "up" | "unknown");
    i.loopback = i.loopback || flags & IFF_LOOPBACK != 0;
}

/// Every address per interface, `ip a` style, as (IPv4, IPv6).
fn read_addrs() -> HashMap<String, (Vec<String>, Vec<String>)> {
    use if_addrs::IfAddr;
    let mut out: HashMap<String, (Vec<String>, Vec<String>)> = HashMap::new();
    for a in if_addrs::get_if_addrs().unwrap_or_default() {
        let entry = out.entry(a.name).or_default();
        match a.addr {
            IfAddr::V4(v) => entry.0.push(format!("{}/{}", v.ip, v.prefixlen)),
            IfAddr::V6(v) => entry.1.push(format!("{}/{}", v.ip, v.prefixlen)),
        }
    }
    for (_, v6) in out.values_mut() {
        // Stable: within each group the system's order is kept.
        v6.sort_by_key(|a| a.starts_with("fe80:"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(rx: u64, tx: u64) -> Counters {
        Counters {
            rx_bytes: rx,
            tx_bytes: tx,
            ..Default::default()
        }
    }

    #[test]
    fn parses_proc_format_including_no_space_after_colon() {
        let text = "Inter-|   Receive  |  Transmit\n face |bytes packets|bytes\n\
            lo: 1 2 3 4 0 0 0 0 5 6 7 8 0 0 0 0\n\
            eth0:100 2 0 0 0 0 0 0 200 3 0 0 0 0 0 0\n\
            bad: 1 2 3\n";
        let p = parse_proc_net_dev(text);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].0, "lo");
        assert_eq!(p[0].1.rx_drop, 4);
        assert_eq!(p[0].1.tx_bytes, 5);
        assert_eq!(p[0].1.tx_drop, 8);
        assert_eq!(p[1].0, "eth0");
        assert_eq!(p[1].1.rx_bytes, 100);
    }

    #[test]
    fn rollup_sums_interfaces_and_excludes_loopback() {
        let mut m = Monitor::default();
        m.apply(
            vec![
                ("lo".into(), c(0, 0)),
                ("a".into(), c(0, 0)),
                ("b".into(), c(0, 0)),
            ],
            0.0,
        );
        m.apply(
            vec![
                ("lo".into(), c(9000, 9000)),
                ("a".into(), c(100, 10)),
                ("b".into(), c(300, 30)),
            ],
            2.0,
        );
        assert_eq!(m.master.rx.rate, 200.0);
        assert_eq!(m.master.tx.rate, 20.0);
        assert_eq!(m.master.rx.session, 400);
        let lo = m.ifaces.iter().find(|i| i.name == "lo").unwrap();
        assert_eq!(lo.series.rx.rate, 4500.0);
    }

    #[test]
    fn counter_reset_does_not_spike() {
        let mut m = Monitor::default();
        m.apply(vec![("a".into(), c(1_000_000, 0))], 0.0);
        m.apply(vec![("a".into(), c(50, 0))], 1.0);
        assert_eq!(m.master.rx.rate, 0.0);
        m.apply(vec![("a".into(), c(150, 0))], 1.0);
        assert_eq!(m.master.rx.rate, 100.0);
    }

    #[test]
    fn new_interface_mid_run_is_baselined() {
        let mut m = Monitor::default();
        m.apply(vec![("a".into(), c(0, 0))], 0.0);
        m.apply(
            vec![("a".into(), c(0, 0)), ("vpn".into(), c(5_000_000, 0))],
            1.0,
        );
        assert_eq!(m.master.rx.rate, 0.0);
        assert_eq!(m.master.counters.rx_bytes, 5_000_000);
    }

    #[test]
    fn removed_interface_disappears_but_rollup_history_stays() {
        let mut m = Monitor::default();
        m.apply(vec![("a".into(), c(0, 0)), ("b".into(), c(0, 0))], 0.0);
        m.apply(vec![("a".into(), c(100, 0)), ("b".into(), c(100, 0))], 1.0);
        m.apply(vec![("a".into(), c(100, 0))], 1.0);
        assert_eq!(m.ifaces.len(), 1);
        assert_eq!(m.master.rx.session, 200);
        assert_eq!(m.master.rx.history.len(), 2);
    }

    #[test]
    fn missed_read_uses_the_whole_gap_and_does_not_spike_the_peak() {
        let mut m = Monitor::default();
        m.apply(vec![("a".into(), c(0, 0))], 0.0);
        m.apply(vec![("a".into(), c(1000, 0))], 1.0);
        assert_eq!(m.master.rx.rate, 1000.0);
        assert_eq!(m.master.rx.peak, 1000.0);
        // The failed tick records nothing. The next success covers both seconds.
        m.apply(vec![("a".into(), c(2000, 0))], 2.0);
        assert_eq!(m.master.rx.rate, 500.0);
        assert_eq!(m.master.rx.peak, 1000.0);
    }
}
