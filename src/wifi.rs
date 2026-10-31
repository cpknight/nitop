//! Wi-Fi link state, asked directly of the kernel over nl80211 (the interface `iw` uses).
//! Needs no privileges and no external tools. Every field is best effort.

use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const NETLINK_GENERIC: i32 = 16;
const GENL_ID_CTRL: u16 = 0x10;
const CTRL_CMD_GETFAMILY: u8 = 3;
const CTRL_ATTR_FAMILY_ID: u16 = 1;
const CTRL_ATTR_FAMILY_NAME: u16 = 2;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_REQUEST: u16 = 1;
const NLM_F_MULTI: u16 = 2;
const NLM_F_DUMP: u16 = 0x300;
const NLA_TYPE_MASK: u16 = 0x3fff;
const HEADER_LEN: usize = 16 + 4; // nlmsghdr + genlmsghdr

const CMD_GET_INTERFACE: u8 = 5;
const CMD_GET_STATION: u8 = 17;
const CMD_GET_SCAN: u8 = 32;
const ATTR_IFINDEX: u16 = 3;
const ATTR_STA_INFO: u16 = 21;
const ATTR_WIPHY_FREQ: u16 = 38;
const ATTR_BSS: u16 = 47;
const ATTR_SSID: u16 = 52;
const ATTR_CHANNEL_WIDTH: u16 = 159;
const BSS_INFORMATION_ELEMENTS: u16 = 6;
const BSS_STATUS: u16 = 9;
const IE_SSID: u8 = 0;
const STA_SIGNAL: u16 = 7;
const STA_TX_BITRATE: u16 = 8;
const RATE_BITRATE: u16 = 1; // 100 kb/s units
const RATE_MCS: u16 = 2;
const RATE_BITRATE32: u16 = 5;
const RATE_VHT_MCS: u16 = 6;
const RATE_HE_MCS: u16 = 13;
const RATE_EHT_MCS: u16 = 19;

#[derive(Clone, Default, Debug, PartialEq)]
pub struct WifiLink {
    pub ssid: Option<String>,
    /// The associated BSS advertised an empty or all-zero SSID. Distinct from a
    /// scan that failed or never saw the BSS, which leaves both this and `ssid` unset.
    pub hidden: bool,
    pub freq_mhz: Option<u32>,
    pub width_mhz: Option<u32>,
    pub signal_dbm: Option<i32>,
    pub tx_mbps: Option<f64>,
    pub generation: Option<&'static str>,
}

impl WifiLink {
    /// e.g. "5 GHz ch44 80 MHz"
    pub fn band(&self) -> Option<String> {
        let f = self.freq_mhz?;
        let (band, ch) = band_channel(f)?;
        let mut s = format!("{band} GHz ch{ch}");
        if let Some(w) = self.width_mhz {
            s += &format!(" {w} MHz");
        }
        Some(s)
    }
}

fn band_channel(f: u32) -> Option<(&'static str, u32)> {
    match f {
        2484 => Some(("2.4", 14)),
        2412..=2472 => Some(("2.4", (f - 2407) / 5)),
        // Channel 2 sits below the regular 6 GHz grid.
        5935 => Some(("6", 2)),
        5955..=7115 => Some(("6", (f - 5950) / 5)),
        // Through UNII-4 (channels 181 and 185). 6 GHz starts above this.
        5160..=5925 => Some(("5", (f - 5000) / 5)),
        _ => None,
    }
}

fn is_6ghz(f: u32) -> bool {
    f == 5935 || (5955..=7115).contains(&f)
}

/// Current link for one interface, or `None` if nl80211 can't be reached.
/// Not being associated is `Some` with an empty `ssid`.
pub fn query(ifindex: u32) -> Option<WifiLink> {
    let sock = Socket::open()?;
    let family = sock.resolve_family("nl80211")?;
    let mut link = WifiLink::default();
    let idx = ifindex.to_ne_bytes();

    for msg in sock.request(
        family,
        NLM_F_REQUEST,
        CMD_GET_INTERFACE,
        &attr(ATTR_IFINDEX, &idx),
    )? {
        apply_interface(&mut link, &msg);
    }
    // Station info describes the AP we're associated with; it's absent otherwise.
    let stations = sock.request(
        family,
        NLM_F_REQUEST | NLM_F_DUMP,
        CMD_GET_STATION,
        &attr(ATTR_IFINDEX, &idx),
    );
    if let Some(msg) = stations.as_ref().and_then(|m| m.first()) {
        apply_station(&mut link, msg);
        // Some setups (iwd, for one) don't report the SSID on the interface. The scan
        // entry for the associated AP always carries it, which is where `iw link` looks.
        if link.ssid.is_none() {
            let scan = sock.request(
                family,
                NLM_F_REQUEST | NLM_F_DUMP,
                CMD_GET_SCAN,
                &attr(ATTR_IFINDEX, &idx),
            );
            // A failed or empty scan stays unnamed. Only an empty SSID element is hidden.
            match scan.and_then(|msgs| msgs.iter().find_map(|m| associated_ssid(m))) {
                Some(SsidHit::Named(ssid)) => link.ssid = Some(ssid),
                Some(SsidHit::Hidden) => link.hidden = true,
                None => {}
            }
        }
    }
    Some(link)
}

fn apply_interface(link: &mut WifiLink, payload: &[u8]) {
    for (ty, v) in Attrs(payload) {
        match ty {
            ATTR_SSID if !v.is_empty() => link.ssid = Some(String::from_utf8_lossy(v).into_owned()),
            ATTR_WIPHY_FREQ => link.freq_mhz = read_u32(v),
            ATTR_CHANNEL_WIDTH => link.width_mhz = read_u32(v).and_then(channel_width),
            _ => {}
        }
    }
}

fn apply_station(link: &mut WifiLink, payload: &[u8]) {
    let Some((_, info)) = Attrs(payload).find(|(t, _)| *t == ATTR_STA_INFO) else {
        return;
    };
    for (ty, v) in Attrs(info) {
        match ty {
            STA_SIGNAL => link.signal_dbm = v.first().map(|&b| b as i8 as i32),
            STA_TX_BITRATE => apply_rate(link, v),
            _ => {}
        }
    }
}

#[derive(Debug, PartialEq)]
enum SsidHit {
    Named(String),
    Hidden,
}

/// The SSID in a scan result, if that result is the AP we're associated with.
fn associated_ssid(payload: &[u8]) -> Option<SsidHit> {
    let (_, bss) = Attrs(payload).find(|(t, _)| *t == ATTR_BSS)?;
    Attrs(bss).find(|(t, _)| *t == BSS_STATUS)?;
    let (_, ies) = Attrs(bss).find(|(t, _)| *t == BSS_INFORMATION_ELEMENTS)?;
    let ssid = find_ie(ies, IE_SSID)?;
    // Hidden networks advertise an empty or all-zero SSID. A missing element is
    // not that: the scan simply didn't carry a name.
    if ssid.is_empty() || ssid.iter().all(|&b| b == 0) {
        return Some(SsidHit::Hidden);
    }
    Some(SsidHit::Named(String::from_utf8_lossy(ssid).into_owned()))
}

/// Finds an 802.11 information element (id, length, data).
fn find_ie(mut ies: &[u8], id: u8) -> Option<&[u8]> {
    while let [ie_id, len, rest @ ..] = ies {
        let data = rest.get(..*len as usize)?;
        if *ie_id == id {
            return Some(data);
        }
        ies = &rest[*len as usize..];
    }
    None
}

fn apply_rate(link: &mut WifiLink, rate: &[u8]) {
    let (mut r16, mut r32, mut rank) = (None, None, 0);
    for (ty, v) in Attrs(rate) {
        match ty {
            RATE_BITRATE => r16 = v.get(..2).map(|b| u16::from_ne_bytes([b[0], b[1]]) as u32),
            RATE_BITRATE32 => r32 = read_u32(v),
            RATE_MCS => rank = rank.max(4),
            RATE_VHT_MCS => rank = rank.max(5),
            RATE_HE_MCS => rank = rank.max(6),
            RATE_EHT_MCS => rank = rank.max(7),
            _ => {}
        }
    }
    link.tx_mbps = r32.or(r16).filter(|&r| r > 0).map(|r| r as f64 / 10.0);
    link.generation = match rank {
        4 => Some("Wi-Fi 4"),
        5 => Some("Wi-Fi 5"),
        6 if link.freq_mhz.is_some_and(is_6ghz) => Some("Wi-Fi 6E"),
        6 => Some("Wi-Fi 6"),
        7 => Some("Wi-Fi 7"),
        _ => None,
    };
}

fn channel_width(v: u32) -> Option<u32> {
    match v {
        0 | 1 => Some(20),
        2 => Some(40),
        3 | 4 => Some(80),
        5 => Some(160),
        6 => Some(5),
        7 => Some(10),
        13 => Some(320),
        _ => None,
    }
}

fn read_u32(v: &[u8]) -> Option<u32> {
    Some(u32::from_ne_bytes(v.get(..4)?.try_into().ok()?))
}

/// Encodes one netlink attribute, padded to 4 bytes.
fn attr(ty: u16, value: &[u8]) -> Vec<u8> {
    let len = 4 + value.len();
    let mut out = Vec::with_capacity(len.next_multiple_of(4));
    out.extend_from_slice(&(len as u16).to_ne_bytes());
    out.extend_from_slice(&ty.to_ne_bytes());
    out.extend_from_slice(value);
    out.resize(len.next_multiple_of(4), 0);
    out
}

/// Iterates netlink attributes, stopping quietly at anything malformed.
struct Attrs<'a>(&'a [u8]);

impl<'a> Iterator for Attrs<'a> {
    type Item = (u16, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let b = self.0;
        let len = u16::from_ne_bytes([*b.first()?, *b.get(1)?]) as usize;
        let ty = u16::from_ne_bytes([*b.get(2)?, *b.get(3)?]) & NLA_TYPE_MASK;
        if len < 4 || len > b.len() {
            return None;
        }
        self.0 = b.get(len.next_multiple_of(4)..).unwrap_or(&[]);
        Some((ty, &b[4..len]))
    }
}

/// Splits a netlink datagram into `(type, flags, payload after the generic header)`.
fn split_messages(mut b: &[u8]) -> Vec<(u16, u16, &[u8])> {
    let mut out = Vec::new();
    while b.len() >= 16 {
        let len = u32::from_ne_bytes([b[0], b[1], b[2], b[3]]) as usize;
        if len < 16 || len > b.len() {
            break;
        }
        let ty = u16::from_ne_bytes([b[4], b[5]]);
        let flags = u16::from_ne_bytes([b[6], b[7]]);
        out.push((ty, flags, b.get(HEADER_LEN..len).unwrap_or(&[])));
        b = b.get(len.next_multiple_of(4)..).unwrap_or(&[]);
    }
    out
}

struct Socket(i32);

impl Drop for Socket {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

impl Socket {
    fn open() -> Option<Socket> {
        // SAFETY: plain socket calls; the timeout pointer is valid for the call's duration.
        unsafe {
            let fd = libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                NETLINK_GENERIC,
            );
            if fd < 0 {
                return None;
            }
            let sock = Socket(fd);
            // One stuck command must not stall the poller, which serves every interface.
            let tv = libc::timeval {
                tv_sec: 0,
                tv_usec: 200_000,
            };
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                (&tv as *const libc::timeval).cast(),
                size_of::<libc::timeval>() as u32,
            );
            Some(sock)
        }
    }

    fn resolve_family(&self, name: &str) -> Option<u16> {
        let mut value = name.as_bytes().to_vec();
        value.push(0);
        let msgs = self.request(
            GENL_ID_CTRL,
            NLM_F_REQUEST,
            CTRL_CMD_GETFAMILY,
            &attr(CTRL_ATTR_FAMILY_NAME, &value),
        )?;
        let (_, id) = Attrs(msgs.first()?).find(|(t, _)| *t == CTRL_ATTR_FAMILY_ID)?;
        Some(u16::from_ne_bytes([*id.first()?, *id.get(1)?]))
    }

    /// Sends one request; returns each reply's attribute bytes, or `None` on a kernel error.
    fn request(&self, family: u16, flags: u16, cmd: u8, attrs: &[u8]) -> Option<Vec<Vec<u8>>> {
        let total = HEADER_LEN + attrs.len();
        let mut msg = Vec::with_capacity(total);
        msg.extend_from_slice(&(total as u32).to_ne_bytes());
        msg.extend_from_slice(&family.to_ne_bytes());
        msg.extend_from_slice(&flags.to_ne_bytes());
        msg.extend_from_slice(&1u32.to_ne_bytes()); // seq
        msg.extend_from_slice(&0u32.to_ne_bytes()); // pid: assigned by the kernel
        msg.extend_from_slice(&[cmd, 1, 0, 0]); // genlmsghdr
        msg.extend_from_slice(attrs);

        // SAFETY: both buffers are valid for the lengths passed.
        if unsafe { libc::send(self.0, msg.as_ptr().cast(), msg.len(), 0) } < 0 {
            return None;
        }
        let mut out = Vec::new();
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            let n = unsafe { libc::recv(self.0, buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n <= 0 {
                return None;
            }
            match absorb(&mut out, &buf[..n as usize]) {
                Step::More => {}
                Step::Done => return Some(out),
                Step::Failed => return None,
            }
        }
    }
}

enum Step {
    More,
    Done,
    Failed,
}

/// Folds one netlink datagram into `out`. Multipart replies set `NLM_F_MULTI`
/// and finish with `NLMSG_DONE`; `NLMSG_ERROR` fails the whole request.
fn absorb(out: &mut Vec<Vec<u8>>, datagram: &[u8]) -> Step {
    let mut more = false;
    for (ty, flags, payload) in split_messages(datagram) {
        match ty {
            NLMSG_DONE => return Step::Done,
            NLMSG_ERROR => return Step::Failed,
            _ => {
                more = flags & NLM_F_MULTI != 0;
                out.push(payload.to_vec());
            }
        }
    }
    if more { Step::More } else { Step::Done }
}

const POLL_EVERY: Duration = Duration::from_secs(5);

/// Refreshes Wi-Fi links off the UI thread. A scan dump can take several
/// round trips; the display only reads the last result.
pub struct Poller {
    wanted: Arc<Mutex<HashSet<u32>>>,
    results: Arc<Mutex<HashMap<u32, WifiLink>>>,
}

impl Poller {
    pub fn start() -> Self {
        let wanted = Arc::new(Mutex::new(HashSet::new()));
        let results = Arc::new(Mutex::new(HashMap::new()));
        let wanted_bg = Arc::clone(&wanted);
        let results_bg = Arc::clone(&results);
        thread::spawn(move || {
            loop {
                let indexes: Vec<u32> = wanted_bg
                    .lock()
                    .map(|set| set.iter().copied().collect())
                    .unwrap_or_default();
                if indexes.is_empty() {
                    thread::sleep(Duration::from_millis(200));
                    continue;
                }
                for idx in indexes {
                    if let Some(link) = query(idx)
                        && let Ok(mut map) = results_bg.lock()
                    {
                        map.insert(idx, link);
                    }
                }
                thread::sleep(POLL_EVERY);
            }
        });
        Poller { wanted, results }
    }

    /// Replaces the set of interfaces to refresh. Dropped indexes stop being queried.
    pub fn watch(&self, indexes: impl IntoIterator<Item = u32>) {
        if let Ok(mut set) = self.wanted.lock() {
            set.clear();
            set.extend(indexes);
        }
    }

    pub fn get(&self, ifindex: u32) -> Option<WifiLink> {
        self.results.lock().ok()?.get(&ifindex).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes_round_trip_with_padding() {
        let mut b = attr(1, b"abcde");
        assert_eq!(b.len(), 12);
        b.extend(attr(2, &[9, 0, 0, 0]));
        let got: Vec<_> = Attrs(&b).collect();
        assert_eq!(got, vec![(1, &b"abcde"[..]), (2, &[9, 0, 0, 0][..])]);
    }

    #[test]
    fn malformed_attributes_stop_iteration() {
        assert_eq!(Attrs(&[200, 0, 1, 0, 9]).count(), 0);
        assert_eq!(Attrs(&[0, 0, 1, 0]).count(), 0);
        assert_eq!(Attrs(&[1]).count(), 0);
    }

    #[test]
    fn decodes_ssid_frequency_and_width() {
        let mut p = attr(ATTR_SSID, b"home");
        p.extend(attr(ATTR_WIPHY_FREQ, &5220u32.to_ne_bytes()));
        p.extend(attr(ATTR_CHANNEL_WIDTH, &3u32.to_ne_bytes()));
        let mut l = WifiLink::default();
        apply_interface(&mut l, &p);
        assert_eq!(l.ssid.as_deref(), Some("home"));
        assert_eq!(l.band().as_deref(), Some("5 GHz ch44 80 MHz"));
    }

    #[test]
    fn decodes_negative_signal_and_rate() {
        let mut rate = attr(RATE_BITRATE32, &8660u32.to_ne_bytes());
        rate.extend(attr(RATE_HE_MCS, &[9]));
        let mut info = attr(STA_SIGNAL, &[(-59i8) as u8]);
        info.extend(attr(STA_TX_BITRATE, &rate));
        let mut l = WifiLink {
            freq_mhz: Some(5220),
            ..Default::default()
        };
        apply_station(&mut l, &attr(ATTR_STA_INFO, &info));
        assert_eq!(l.signal_dbm, Some(-59));
        assert_eq!(l.tx_mbps, Some(866.0));
        assert_eq!(l.generation, Some("Wi-Fi 6"));
    }

    fn scan_entry(associated: bool, ies: &[u8]) -> Vec<u8> {
        let mut bss = attr(BSS_INFORMATION_ELEMENTS, ies);
        if associated {
            bss.extend(attr(BSS_STATUS, &1u32.to_ne_bytes()));
        }
        attr(ATTR_BSS, &bss)
    }

    #[test]
    fn ssid_comes_only_from_the_associated_scan_entry() {
        let ies = [1, 2, 0xaa, 0xbb, 0, 4, b'h', b'o', b'm', b'e'];
        assert_eq!(
            associated_ssid(&scan_entry(true, &ies)),
            Some(SsidHit::Named("home".into()))
        );
        assert_eq!(associated_ssid(&scan_entry(false, &ies)), None);
    }

    #[test]
    fn hidden_ssid_is_distinct_from_a_truncated_one() {
        assert_eq!(
            associated_ssid(&scan_entry(true, &[0, 0])),
            Some(SsidHit::Hidden)
        );
        assert_eq!(
            associated_ssid(&scan_entry(true, &[0, 3, 0, 0, 0])),
            Some(SsidHit::Hidden)
        );
        assert_eq!(associated_ssid(&scan_entry(true, &[0, 9, b'x'])), None);
    }

    #[test]
    fn channels_for_each_band() {
        assert_eq!(band_channel(2412), Some(("2.4", 1)));
        assert_eq!(band_channel(2484), Some(("2.4", 14)));
        assert_eq!(band_channel(5180), Some(("5", 36)));
        assert_eq!(band_channel(5905), Some(("5", 181)));
        assert_eq!(band_channel(5925), Some(("5", 185)));
        assert_eq!(band_channel(5935), Some(("6", 2)));
        assert_eq!(band_channel(5955), Some(("6", 1)));
        assert_eq!(band_channel(100), None);
    }

    #[test]
    fn wifi_6e_is_only_the_6ghz_band() {
        let mut link = WifiLink {
            freq_mhz: Some(5905),
            ..Default::default()
        };
        apply_rate(&mut link, &attr(RATE_HE_MCS, &[9]));
        assert_eq!(link.generation, Some("Wi-Fi 6"));
        link.freq_mhz = Some(5955);
        apply_rate(&mut link, &attr(RATE_HE_MCS, &[9]));
        assert_eq!(link.generation, Some("Wi-Fi 6E"));
        link.freq_mhz = Some(5935);
        apply_rate(&mut link, &attr(RATE_HE_MCS, &[9]));
        assert_eq!(link.generation, Some("Wi-Fi 6E"));
    }

    fn nlmsg(ty: u16, flags: u16, payload: &[u8]) -> Vec<u8> {
        let len = 16 + payload.len();
        let mut msg = Vec::new();
        msg.extend_from_slice(&(len as u32).to_ne_bytes());
        msg.extend_from_slice(&ty.to_ne_bytes());
        msg.extend_from_slice(&flags.to_ne_bytes());
        msg.extend_from_slice(&1u32.to_ne_bytes());
        msg.extend_from_slice(&0u32.to_ne_bytes());
        msg.extend_from_slice(payload);
        msg
    }

    #[test]
    fn multipart_reply_finishes_on_the_second_datagram() {
        let attrs = attr(ATTR_SSID, b"home");
        let mut body = vec![0, 1, 0, 0];
        body.extend_from_slice(&attrs);
        let mut out = Vec::new();
        assert!(matches!(
            absorb(&mut out, &nlmsg(family_type(), NLM_F_MULTI, &body)),
            Step::More
        ));
        assert_eq!(out, vec![attrs.clone()]);
        assert!(matches!(
            absorb(&mut out, &nlmsg(NLMSG_DONE, 0, &[])),
            Step::Done
        ));
        assert_eq!(out, vec![attrs]);
        assert!(matches!(
            absorb(&mut out, &nlmsg(NLMSG_ERROR, 0, &[])),
            Step::Failed
        ));
    }

    fn family_type() -> u16 {
        32
    }
}
