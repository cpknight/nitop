//! ZeroTier network details, best effort. `zerotier-cli` is asked first because it reports
//! live status, but its auth token is usually root-only. The daemon's network files are
//! normally world-readable and give the name and ID, so they are the fallback. Any failure
//! just means less detail: nothing here may disturb the display.

use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const CLI: &str = "zerotier-cli";
const NETWORKS_DIR: &str = "/var/lib/zerotier-one/networks.d";
const POLL_EVERY: Duration = Duration::from_secs(30);
const CLI_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Network {
    pub id: String,
    pub name: Option<String>,
    /// Only known from the CLI.
    pub status: Option<String>,
    pub kind: Option<String>,
    pub device: Option<String>,
    pub mac: Option<String>,
}

/// What we can say about a ZeroTier interface.
#[derive(Clone, Debug, PartialEq)]
pub enum Info {
    Network(Network),
    Unavailable(&'static str),
}

#[derive(Debug, PartialEq)]
enum CliError {
    NotInstalled,
    NoAccess,
    Failed,
}

/// Asks the CLI for the joined networks, without ever blocking longer than `timeout`.
fn query_cli(program: &str, timeout: Duration) -> Result<Vec<Network>, CliError> {
    let mut child = Command::new(program)
        .args(["-j", "listnetworks"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => CliError::NotInstalled,
            std::io::ErrorKind::PermissionDenied => CliError::NoAccess,
            _ => CliError::Failed,
        })?;

    // Read on threads, so a full pipe can't stall the child while we wait for it.
    let drain = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        })
    };
    let out = child.stdout.take().map(|p| drain(Box::new(p)));
    let err = child.stderr.take().map(|p| drain(Box::new(p)));

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CliError::Failed);
            }
        }
    };
    let join =
        |h: Option<thread::JoinHandle<String>>| h.and_then(|h| h.join().ok()).unwrap_or_default();
    let (stdout, stderr) = (join(out), join(err));
    if !status.success() {
        let denied = stderr.contains("authtoken") || stdout.contains("authtoken");
        return Err(if denied {
            CliError::NoAccess
        } else {
            CliError::Failed
        });
    }
    parse_cli(&stdout).ok_or(CliError::Failed)
}

fn parse_cli(json: &str) -> Option<Vec<Network>> {
    let value: Value = serde_json::from_str(json).ok()?;
    let text = |n: &Value, key: &str| {
        n.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(
        value
            .as_array()?
            .iter()
            .filter_map(|n| {
                Some(Network {
                    id: text(n, "nwid").or_else(|| text(n, "id"))?,
                    name: text(n, "name"),
                    status: text(n, "status"),
                    kind: text(n, "type"),
                    device: text(n, "portDeviceName"),
                    mac: text(n, "mac"),
                })
            })
            .collect(),
    )
}

/// Reads the daemon's cached network configs. This is its private format, so anything
/// unexpected is skipped.
fn read_network_files(dir: &str) -> Vec<Network> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Network> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.ends_with(".conf") && !name.ends_with(".local.conf")
        })
        .filter_map(|e| fs::read(e.path()).ok())
        .filter_map(|bytes| parse_network_file(&bytes))
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn parse_network_file(bytes: &[u8]) -> Option<Network> {
    let mut fields: HashMap<&[u8], &[u8]> = HashMap::new();
    for line in bytes.split(|&b| b == b'\n') {
        if let Some(eq) = line.iter().position(|&b| b == b'=') {
            fields.insert(&line[..eq], &line[eq + 1..]);
        }
    }
    let text = |key: &[u8]| -> Option<String> {
        let v = unescape(fields.get(key)?);
        (!v.is_empty()).then(|| String::from_utf8_lossy(&v).into_owned())
    };
    let id = text(b"nwid")?;
    let nwid = u64::from_str_radix(&id, 16).ok()?;
    let node = text(b"id").and_then(|s| u64::from_str_radix(&s, 16).ok());
    let kind = text(b"t").and_then(|s| u64::from_str_radix(&s, 16).ok());
    Some(Network {
        id: format!("{nwid:016x}"),
        name: text(b"n"),
        status: None,
        kind: kind.and_then(|t| match t {
            0 => Some("PRIVATE".to_string()),
            1 => Some("PUBLIC".to_string()),
            _ => None,
        }),
        device: None,
        mac: node.map(|n| format_mac(derive_mac(n, nwid))),
    })
}

/// Undoes ZeroTier's Dictionary escaping.
fn unescape(v: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len());
    let mut bytes = v.iter();
    while let Some(&b) = bytes.next() {
        if b != b'\\' {
            out.push(b);
            continue;
        }
        match bytes.next() {
            Some(b'r') => out.push(b'\r'),
            Some(b'n') => out.push(b'\n'),
            Some(b'0') => out.push(0),
            Some(b'e') => out.push(b'='),
            Some(&other) => out.push(other),
            None => {}
        }
    }
    out
}

/// The interface MAC ZeroTier derives from a node address and network ID, which ties a
/// network's file to its interface.
fn derive_mac(node: u64, nwid: u64) -> u64 {
    let first = (nwid & 0xfe) | 0x02;
    let first = if first == 0x52 { 0x32 } else { first };
    let mut m = (first << 40) | (node & 0xff_ffff_ffff);
    m ^= ((nwid >> 8) & 0xff) << 32;
    m ^= ((nwid >> 16) & 0xff) << 24;
    m ^= ((nwid >> 24) & 0xff) << 16;
    m ^= ((nwid >> 32) & 0xff) << 8;
    m ^= (nwid >> 40) & 0xff;
    m
}

fn format_mac(m: u64) -> String {
    let b = m.to_be_bytes();
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        b[2], b[3], b[4], b[5], b[6], b[7]
    )
}

#[derive(Clone, Debug, Default, PartialEq)]
enum State {
    #[default]
    Pending,
    Networks(Vec<Network>),
    Unavailable(&'static str),
}

fn fetch(program: &str, dir: &str, timeout: Duration) -> State {
    let error = match query_cli(program, timeout) {
        Ok(networks) => return State::Networks(networks),
        Err(e) => e,
    };
    let files = read_network_files(dir);
    if !files.is_empty() {
        return State::Networks(files);
    }
    State::Unavailable(match error {
        CliError::NotInstalled => "zerotier-cli not found",
        CliError::NoAccess => "no access to zerotier-cli",
        CliError::Failed => "zerotier-cli not responding",
    })
}

impl State {
    fn lookup(&self, device: &str, mac: Option<&str>) -> Option<Info> {
        let networks = match self {
            State::Pending => return None,
            State::Unavailable(why) => return Some(Info::Unavailable(why)),
            State::Networks(n) => n,
        };
        let same_mac =
            |n: &&Network| matches!((&n.mac, mac), (Some(a), Some(b)) if a.eq_ignore_ascii_case(b));
        let found = networks
            .iter()
            .find(|n| n.device.as_deref() == Some(device))
            .or_else(|| networks.iter().find(same_mac))
            // One network and one way to be wrong: this must be it.
            .or(if networks.len() == 1 {
                networks.first()
            } else {
                None
            });
        Some(found.map_or(Info::Unavailable("no matching network"), |n| {
            Info::Network(n.clone())
        }))
    }
}

/// Polls on its own thread, since the CLI can block. Started only once a ZeroTier
/// interface is seen, so nobody else ever runs it.
pub struct Poller(Arc<Mutex<State>>);

impl Poller {
    pub fn start() -> Self {
        let state = Arc::new(Mutex::new(State::Pending));
        let shared = Arc::clone(&state);
        thread::spawn(move || {
            loop {
                let fresh = fetch(CLI, NETWORKS_DIR, CLI_TIMEOUT);
                if let Ok(mut s) = shared.lock() {
                    *s = fresh;
                }
                thread::sleep(POLL_EVERY);
            }
        });
        Poller(state)
    }

    /// `None` until the first poll finishes.
    pub fn lookup(&self, device: &str, mac: Option<&str>) -> Option<Info> {
        self.0.lock().ok()?.lookup(device, mac)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const LIST: &str = r#"[{"id":"8056c2e21c000001","nwid":"8056c2e21c000001","name":"office",
        "status":"OK","type":"PRIVATE","portDeviceName":"ztabc","mac":"36:aa:bb:cc:dd:ee",
        "assignedAddresses":["192.0.2.1/24"]},
        {"nwid":"1111111111111111","name":"","status":"REQUESTING_CONFIGURATION","type":"PUBLIC",
        "portDeviceName":"ztdef","mac":"36:00:00:00:00:02"}]"#;

    /// Stands in for zerotier-cli, with a given exit status, stdout and stderr.
    fn fake_cli(dir: &std::path::Path, body: &str) -> String {
        let path = dir.join("fake-cli");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    const SHORT: Duration = Duration::from_secs(3);

    #[test]
    fn parses_cli_output_and_treats_empty_names_as_missing() {
        let nets = parse_cli(LIST).unwrap();
        assert_eq!(nets.len(), 2);
        assert_eq!(nets[0].name.as_deref(), Some("office"));
        assert_eq!(nets[0].status.as_deref(), Some("OK"));
        assert_eq!(nets[0].kind.as_deref(), Some("PRIVATE"));
        assert_eq!(nets[0].device.as_deref(), Some("ztabc"));
        assert_eq!(nets[1].name, None);
    }

    #[test]
    fn bad_cli_output_is_rejected() {
        assert_eq!(parse_cli("not json"), None);
        assert_eq!(parse_cli(r#"{"not":"a list"}"#), None);
        assert_eq!(parse_cli(r#"[{"name":"no id"}]"#), Some(vec![]));
    }

    #[test]
    fn network_file_gives_name_id_type_and_mac() {
        let bytes = b"v=0000000000000007\nnwid=8056c2e21c000001\nid=89e92ceee5\n\
            t=0\nn=my\\enet\\\\x\nC=\xff\xfe\x01binary\n";
        let n = parse_network_file(bytes).unwrap();
        assert_eq!(n.id, "8056c2e21c000001");
        assert_eq!(n.name.as_deref(), Some("my=net\\x"));
        assert_eq!(n.kind.as_deref(), Some("PRIVATE"));
        assert_eq!(n.status, None);
        assert!(n.mac.is_some());
    }

    #[test]
    fn unusable_network_files_are_skipped() {
        assert_eq!(parse_network_file(b""), None);
        assert_eq!(parse_network_file(b"n=no id\n"), None);
        assert_eq!(parse_network_file(b"nwid=nothex\n"), None);
    }

    #[test]
    fn mac_derivation_follows_zerotiers_rules() {
        // Multicast bit clear and locally administered bit set, whatever the network ID.
        assert_eq!(derive_mac(0, 0) >> 40, 0x02);
        assert_eq!(derive_mac(0, 0xff) >> 40, 0xfe);
        // 0x52 is reserved, so it is skipped.
        assert_eq!(derive_mac(0, 0x52) >> 40, 0x32);
        // The node address shows through, XORed with the network ID's bytes.
        let nwid = 0x8056_c2e2_1c00_0001;
        assert_eq!(
            derive_mac(0x89_e92c_eee5, nwid) & 0xff_ffff_ffff,
            0x89_e92c_eee5 ^ 0x00_001c_e2c2
        );
        assert_eq!(format_mac(0x36aa_bbcc_ddee), "36:aa:bb:cc:dd:ee");
    }

    #[test]
    fn cli_success_failure_and_timeout() {
        let tmp = tempfile::tempdir().unwrap();
        let ok = fake_cli(tmp.path(), &format!("cat <<'X'\n{LIST}\nX"));
        assert_eq!(query_cli(&ok, SHORT).unwrap().len(), 2);

        let denied = fake_cli(tmp.path(), "echo 'authtoken.secret not found' >&2; exit 2");
        assert_eq!(query_cli(&denied, SHORT), Err(CliError::NoAccess));

        let broken = fake_cli(tmp.path(), "echo boom >&2; exit 1");
        assert_eq!(query_cli(&broken, SHORT), Err(CliError::Failed));

        let garbage = fake_cli(tmp.path(), "echo not-json");
        assert_eq!(query_cli(&garbage, SHORT), Err(CliError::Failed));

        assert_eq!(
            query_cli("/nonexistent/zerotier-cli", SHORT),
            Err(CliError::NotInstalled)
        );
    }

    #[test]
    fn hung_cli_is_killed_at_the_timeout() {
        let tmp = tempfile::tempdir().unwrap();
        let hung = fake_cli(tmp.path(), "exec sleep 30");
        let start = Instant::now();
        assert_eq!(
            query_cli(&hung, Duration::from_millis(200)),
            Err(CliError::Failed)
        );
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn falls_back_to_files_then_reports_why() {
        let tmp = tempfile::tempdir().unwrap();
        let denied = fake_cli(tmp.path(), "echo authtoken >&2; exit 2");
        let dir = tmp.path().join("networks.d");
        fs::create_dir(&dir).unwrap();
        let d = dir.to_str().unwrap();

        assert_eq!(
            fetch(&denied, d, SHORT),
            State::Unavailable("no access to zerotier-cli")
        );
        assert_eq!(
            fetch("/nonexistent/cli", "/nonexistent/dir", SHORT),
            State::Unavailable("zerotier-cli not found")
        );

        fs::write(
            dir.join("8056c2e21c000001.conf"),
            "nwid=8056c2e21c000001\nn=office\n",
        )
        .unwrap();
        fs::write(dir.join("8056c2e21c000001.local.conf"), "allowManaged=1\n").unwrap();
        match fetch(&denied, d, SHORT) {
            State::Networks(n) => {
                assert_eq!(n.len(), 1);
                assert_eq!(n[0].name.as_deref(), Some("office"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn lookup_matches_device_then_mac_then_a_lone_network() {
        let two = State::Networks(parse_cli(LIST).unwrap());
        let id = |i: Option<Info>| match i {
            Some(Info::Network(n)) => n.id,
            other => panic!("{other:?}"),
        };
        assert_eq!(id(two.lookup("ztdef", None)), "1111111111111111");
        assert_eq!(
            id(two.lookup("zzz", Some("36:AA:BB:CC:DD:EE"))),
            "8056c2e21c000001"
        );
        assert_eq!(
            two.lookup("zzz", Some("00:00:00:00:00:00")),
            Some(Info::Unavailable("no matching network"))
        );

        let lone = State::Networks(vec![Network {
            id: "a".into(),
            ..Default::default()
        }]);
        assert_eq!(id(lone.lookup("anything", None)), "a");
        assert_eq!(State::Pending.lookup("x", None), None);
    }
}
