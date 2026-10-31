//! What an interface is: its kind, hardware and link details, from sysfs and the hwdata
//! ID databases.

use crate::wifi::WifiLink;
use crate::zerotier;
use std::fs;
use std::path::Path;

const IFF_TUN: u32 = 0x1;
const IFF_TAP: u32 = 0x2;

/// Where system information is read from. Separate so tests can use a fake tree.
struct Roots<'a> {
    net: &'a str,
    pci_ids: &'a str,
    usb_ids: &'a str,
}

const SYSTEM: Roots = Roots {
    net: "/sys/class/net",
    pci_ids: "/usr/share/hwdata/pci.ids",
    usb_ids: "/usr/share/hwdata/usb.ids",
};

#[derive(Clone, Default)]
pub struct Details {
    pub kind: String,
    pub hardware: Option<String>,
    pub driver: Option<String>,
    pub mac: Option<String>,
    pub mtu: Option<u32>,
    pub duplex: Option<String>,
    /// Link speed in Mb/s, on Ethernet.
    pub speed: Option<u32>,
    /// With prefix length, e.g. `192.0.2.1/24`.
    pub ipv4: Vec<String>,
    /// With prefix length, global addresses before link-local ones.
    pub ipv6: Vec<String>,
    pub zerotier: Option<zerotier::Info>,
    /// Present on Wi-Fi interfaces.
    pub wifi: Option<WifiLink>,
}

/// Inputs for `classify`, kept apart from sysfs so each rule can be tested.
#[derive(Default)]
struct Traits<'a> {
    name: &'a str,
    arphrd: u32,
    devtype: &'a str,
    has_device: bool,
    wireless: bool,
    tun_flags: Option<u32>,
}

fn classify(t: &Traits) -> String {
    if t.arphrd == 772 {
        return "Loopback".into();
    }
    if t.wireless || t.devtype == "wlan" {
        return "Wi-Fi".into();
    }
    let by_devtype = match t.devtype {
        "bridge" => "Bridge",
        "bond" => "Bond",
        "team" => "Team",
        "vlan" => "VLAN",
        "veth" => "Virtual Ethernet",
        "wireguard" => "WireGuard VPN",
        "vxlan" => "VXLAN",
        "geneve" => "Geneve",
        "macvlan" => "MACVLAN",
        "ipvlan" => "IPVLAN",
        "wwan" => "Cellular",
        "ppp" => "PPP",
        _ => "",
    };
    if !by_devtype.is_empty() {
        return by_devtype.into();
    }
    if let Some(flags) = t.tun_flags {
        let layer = if flags & IFF_TAP != 0 {
            "TAP"
        } else if flags & IFF_TUN != 0 {
            "TUN"
        } else {
            "tunnel"
        };
        // The kernel can't tell who owns a TUN/TAP device; the name is the only hint.
        let n = t.name;
        let product = if n.starts_with("zt") {
            "ZeroTier"
        } else if n.starts_with("tailscale") {
            "Tailscale"
        } else if n.starts_with("wg") {
            "WireGuard"
        } else if n.starts_with("nordlynx") {
            "NordVPN"
        } else {
            return format!("{layer} tunnel");
        };
        return format!("{product} VPN ({layer})");
    }
    match t.arphrd {
        1 if t.has_device => "Ethernet".into(),
        1 => "Virtual".into(),
        32 => "InfiniBand".into(),
        512 => "PPP".into(),
        768 | 769 => "IP tunnel".into(),
        776 => "IPv6-in-IPv4 tunnel".into(),
        778 | 784 => "GRE tunnel".into(),
        65534 => "Tunnel".into(),
        n => format!("Link type {n}"),
    }
}

/// Everything that can't change while the interface exists, read once.
pub fn read_static(name: &str) -> Details {
    read_static_in(&SYSTEM, name)
}

fn read_static_in(roots: &Roots, name: &str) -> Details {
    let dir = format!("{}/{name}", roots.net);
    let dev = format!("{dir}/device");
    let read = |file: &str| read_trim(&format!("{dir}/{file}"));
    let uevent = read("uevent").unwrap_or_default();
    let devtype = uevent
        .lines()
        .find_map(|l| l.strip_prefix("DEVTYPE="))
        .unwrap_or("");
    let has_device = Path::new(&dev).exists();
    let tun_flags = read("tun_flags").and_then(|s| parse_hex(&s));
    let kind = classify(&Traits {
        name,
        arphrd: read("type").and_then(|s| s.parse().ok()).unwrap_or(0),
        devtype,
        has_device,
        wireless: Path::new(&format!("{dir}/wireless")).exists()
            || Path::new(&format!("{dir}/phy80211")).exists(),
        tun_flags,
    });
    Details {
        kind,
        hardware: has_device.then(|| hardware_name(roots, &dev)).flatten(),
        driver: has_device
            .then(|| link_name(&format!("{dev}/driver")))
            .flatten(),
        ..Default::default()
    }
}

/// Updates the parts that can change: address, MTU and duplex.
/// The Wi-Fi link is filled in by the caller from a background poller, because a
/// scan dump must not run on the thread that draws the display.
pub fn refresh(name: &str, d: &mut Details) {
    refresh_in(&SYSTEM, name, d)
}

pub fn ifindex(name: &str) -> Option<u32> {
    read_trim(&format!("/sys/class/net/{name}/ifindex"))?
        .parse()
        .ok()
}

fn refresh_in(roots: &Roots, name: &str, d: &mut Details) {
    let dir = format!("{}/{name}", roots.net);
    let read = |file: &str| read_trim(&format!("{dir}/{file}"));
    d.mac = read("address").filter(|m| m != "00:00:00:00:00:00" && !m.is_empty());
    d.mtu = read("mtu").and_then(|s| s.parse().ok());
    // Only meaningful on real wired links; tunnels report a made-up "full".
    d.duplex = (d.kind == "Ethernet")
        .then(|| read("duplex"))
        .flatten()
        .filter(|s| s == "full" || s == "half");
    // Negative or unreadable with no link. Virtual devices report made-up speeds.
    d.speed = (d.kind == "Ethernet")
        .then(|| read("speed"))
        .flatten()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|&s| s > 0)
        .map(|s| s as u32);
}

fn read_trim(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn parse_hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

fn link_name(path: &str) -> Option<String> {
    let target = fs::read_link(path).ok()?;
    Some(target.file_name()?.to_string_lossy().into_owned())
}

fn hardware_name(roots: &Roots, dev: &str) -> Option<String> {
    match link_name(&format!("{dev}/subsystem"))?.as_str() {
        "pci" => {
            let id = |f: &str| {
                read_trim(&format!("{dev}/{f}")).map(|s| s.trim_start_matches("0x").to_lowercase())
            };
            let (vendor, device) = (id("vendor")?, id("device")?);
            let subsystem = id("subsystem_vendor").zip(id("subsystem_device"));
            let ids = fs::read_to_string(roots.pci_ids).unwrap_or_default();
            Some(
                lookup_ids(&ids, &vendor, &device, subsystem.as_ref())
                    .unwrap_or(format!("PCI {vendor}:{device}")),
            )
        }
        "usb" => {
            // `device` points at the USB interface; descriptors live on its parent.
            let parent = format!("{dev}/..");
            let desc = |f: &str| read_trim(&format!("{parent}/{f}")).filter(|s| !s.is_empty());
            if let Some(product) = desc("product") {
                return Some(match desc("manufacturer") {
                    Some(m) if !product.starts_with(&m) => format!("{m} {product} (USB)"),
                    _ => format!("{product} (USB)"),
                });
            }
            let (vendor, device) = (desc("idVendor")?, desc("idProduct")?);
            let ids = fs::read_to_string(roots.usb_ids).unwrap_or_default();
            let name =
                lookup_ids(&ids, &vendor, &device, None).unwrap_or(format!("{vendor}:{device}"));
            Some(format!("{name} (USB)"))
        }
        _ => None,
    }
}

/// Finds a name in pci.ids / usb.ids format. The subsystem entry wins when present, since
/// it names the actual card rather than the chip family.
fn lookup_ids(
    text: &str,
    vendor: &str,
    device: &str,
    subsystem: Option<&(String, String)>,
) -> Option<String> {
    let mut vendor_name = None;
    let mut device_name = None;
    let sub_key = subsystem.map(|(v, d)| format!("{v} {d}"));
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("\t\t") {
            if device_name.is_some() && sub_key.as_deref().is_some_and(|k| rest.starts_with(k)) {
                let sub = rest.split_once("  ")?.1;
                return Some(join_vendor(vendor_name?, sub));
            }
        } else if let Some(rest) = line.strip_prefix('\t') {
            if vendor_name.is_none() {
                continue;
            }
            if device_name.is_some() {
                break; // past our device's subsystem lines
            }
            if let Some(name) = rest.strip_prefix(device).and_then(|r| r.strip_prefix("  ")) {
                device_name = Some(name);
            }
        } else if vendor_name.is_some() {
            break; // next vendor
        } else if let Some(name) = line.strip_prefix(vendor).and_then(|r| r.strip_prefix("  ")) {
            vendor_name = Some(name);
        }
    }
    Some(join_vendor(vendor_name?, device_name?))
}

fn join_vendor(vendor: &str, model: &str) -> String {
    let short = vendor
        .trim_end_matches(", Inc.")
        .trim_end_matches(" Inc.")
        .trim_end_matches(" Corporation")
        .trim_end_matches(" Co., Ltd.");
    format!("{short} {model}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDS: &str = "# comment\n\
        10ec  Realtek Semiconductor Co., Ltd.\n\
        \t8125  RTL8125 2.5GbE Controller\n\
        8086  Intel Corporation\n\
        \t51f0  Other Chip\n\
        \t\t8086 0094  Wrong Device Card\n\
        \t51f1  Raptor Lake PCH CNVi WiFi\n\
        \t\t8086 0090  Wi-Fi 6E AX211 Other\n\
        \t\t8086 0094  Wi-Fi 6E AX211 160MHz\n\
        \t51f2  Later Chip\n\
        8087  Intel Next\n\
        \t51f1  Must Not Match\n";

    fn sub(v: &str, d: &str) -> Option<(String, String)> {
        Some((v.into(), d.into()))
    }

    #[test]
    fn subsystem_name_wins_and_vendor_is_shortened() {
        let s = sub("8086", "0094");
        assert_eq!(
            lookup_ids(IDS, "8086", "51f1", s.as_ref()).unwrap(),
            "Intel Wi-Fi 6E AX211 160MHz"
        );
    }

    #[test]
    fn falls_back_to_device_name_without_a_subsystem_match() {
        let s = sub("1234", "5678");
        assert_eq!(
            lookup_ids(IDS, "8086", "51f1", s.as_ref()).unwrap(),
            "Intel Raptor Lake PCH CNVi WiFi"
        );
        assert_eq!(
            lookup_ids(IDS, "10ec", "8125", None).unwrap(),
            "Realtek Semiconductor RTL8125 2.5GbE Controller"
        );
    }

    #[test]
    fn unknown_ids_are_none() {
        assert_eq!(lookup_ids(IDS, "8086", "ffff", None), None);
        assert_eq!(lookup_ids(IDS, "ffff", "51f1", None), None);
        assert_eq!(lookup_ids("", "8086", "51f1", None), None);
    }

    /// A throwaway sysfs tree, built the way the kernel lays it out: `device` and its
    /// `driver` and `subsystem` entries are symlinks.
    struct FakeSys {
        tmp: tempfile::TempDir,
    }

    impl FakeSys {
        fn new() -> Self {
            let s = FakeSys {
                tmp: tempfile::tempdir().unwrap(),
            };
            fs::create_dir_all(s.path("net")).unwrap();
            fs::write(s.path("pci.ids"), IDS).unwrap();
            fs::write(
                s.path("usb.ids"),
                "0bda  Realtek\n\t8153  RTL8153 Ethernet\n",
            )
            .unwrap();
            s
        }

        fn path(&self, rel: &str) -> String {
            self.tmp.path().join(rel).to_string_lossy().into_owned()
        }

        fn write(&self, rel: &str, content: &str) {
            let p = self.path(rel);
            fs::create_dir_all(Path::new(&p).parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }

        fn iface(&self, name: &str, files: &[(&str, &str)]) {
            for (file, content) in files {
                self.write(&format!("net/{name}/{file}"), content);
            }
        }

        /// Creates a device directory, attached to `name` and to a bus and driver.
        fn device(&self, name: &str, dev: &str, bus: &str, driver: &str) {
            let d = self.path(dev);
            fs::create_dir_all(&d).unwrap();
            for target in [format!("bus/{bus}"), format!("drivers/{driver}")] {
                fs::create_dir_all(self.path(&target)).unwrap();
            }
            symlink(self.path(&format!("bus/{bus}")), format!("{d}/subsystem")).unwrap();
            symlink(
                self.path(&format!("drivers/{driver}")),
                format!("{d}/driver"),
            )
            .unwrap();
            symlink(&d, self.path(&format!("net/{name}/device"))).unwrap();
        }

        fn read(&self, name: &str) -> Details {
            let (net, pci_ids, usb_ids) =
                (self.path("net"), self.path("pci.ids"), self.path("usb.ids"));
            let roots = Roots {
                net: &net,
                pci_ids: &pci_ids,
                usb_ids: &usb_ids,
            };
            let mut d = read_static_in(&roots, name);
            refresh_in(&roots, name, &mut d);
            d
        }
    }

    use std::os::unix::fs::symlink;

    #[test]
    fn pci_ethernet_reads_card_driver_and_link() {
        let s = FakeSys::new();
        s.iface(
            "eth0",
            &[
                ("type", "1\n"),
                ("uevent", "INTERFACE=eth0\n"),
                ("address", "02:00:00:00:00:01\n"),
                ("mtu", "1500\n"),
                ("duplex", "full\n"),
                ("speed", "2500\n"),
            ],
        );
        s.device("eth0", "pci/0000:01:00.0", "pci", "r8169");
        s.write("pci/0000:01:00.0/vendor", "0x10ec\n");
        s.write("pci/0000:01:00.0/device", "0x8125\n");
        let d = s.read("eth0");
        assert_eq!(d.kind, "Ethernet");
        assert_eq!(
            d.hardware.as_deref(),
            Some("Realtek Semiconductor RTL8125 2.5GbE Controller")
        );
        assert_eq!(d.driver.as_deref(), Some("r8169"));
        assert_eq!(d.mac.as_deref(), Some("02:00:00:00:00:01"));
        assert_eq!(d.mtu, Some(1500));
        assert_eq!(d.duplex.as_deref(), Some("full"));
        assert_eq!(d.speed, Some(2500));
    }

    #[test]
    fn pci_card_missing_from_the_database_shows_its_ids() {
        let s = FakeSys::new();
        s.iface("eth0", &[("type", "1\n")]);
        s.device("eth0", "pci/x", "pci", "e1000e");
        s.write("pci/x/vendor", "0xABCD\n");
        s.write("pci/x/device", "0x1234\n");
        assert_eq!(s.read("eth0").hardware.as_deref(), Some("PCI abcd:1234"));
    }

    #[test]
    fn usb_adapter_uses_descriptors_on_the_parent_device() {
        let s = FakeSys::new();
        s.iface("enx0", &[("type", "1\n")]);
        s.device("enx0", "usb/1-2/1-2:1.0", "usb", "r8152");
        s.write("usb/1-2/manufacturer", "Realtek\n");
        s.write("usb/1-2/product", "USB 10/100/1000 LAN\n");
        let d = s.read("enx0");
        assert_eq!(d.kind, "Ethernet");
        assert_eq!(
            d.hardware.as_deref(),
            Some("Realtek USB 10/100/1000 LAN (USB)")
        );
        assert_eq!(d.driver.as_deref(), Some("r8152"));
    }

    #[test]
    fn usb_adapter_without_descriptors_falls_back_to_the_database() {
        let s = FakeSys::new();
        s.iface("enx0", &[("type", "1\n")]);
        s.device("enx0", "usb/1-2/1-2:1.0", "usb", "r8152");
        s.write("usb/1-2/idVendor", "0bda\n");
        s.write("usb/1-2/idProduct", "8153\n");
        assert_eq!(
            s.read("enx0").hardware.as_deref(),
            Some("Realtek RTL8153 Ethernet (USB)")
        );
    }

    #[test]
    fn device_on_another_bus_has_a_driver_but_no_name() {
        let s = FakeSys::new();
        s.iface("eth0", &[("type", "1\n")]);
        s.device("eth0", "virtio/virtio0", "virtio", "virtio_net");
        let d = s.read("eth0");
        assert_eq!(d.hardware, None);
        assert_eq!(d.driver.as_deref(), Some("virtio_net"));
    }

    #[test]
    fn virtual_interfaces_have_no_hardware_and_no_duplex() {
        let s = FakeSys::new();
        s.iface(
            "zt0",
            &[
                ("type", "1\n"),
                ("tun_flags", "0x1002\n"),
                ("duplex", "full\n"),
            ],
        );
        s.iface(
            "br0",
            &[
                ("type", "1\n"),
                ("uevent", "DEVTYPE=bridge\nINTERFACE=br0\n"),
            ],
        );
        let zt = s.read("zt0");
        assert_eq!(zt.kind, "ZeroTier VPN (TAP)");
        assert_eq!(zt.hardware, None);
        assert_eq!(zt.duplex, None);
        assert_eq!(zt.speed, None);
        assert_eq!(s.read("br0").kind, "Bridge");
    }

    #[test]
    fn loopback_hides_its_all_zero_mac() {
        let s = FakeSys::new();
        s.iface(
            "lo",
            &[
                ("type", "772\n"),
                ("address", "00:00:00:00:00:00\n"),
                ("mtu", "65536\n"),
            ],
        );
        let d = s.read("lo");
        assert_eq!(d.kind, "Loopback");
        assert_eq!(d.mac, None);
    }

    #[test]
    fn interface_that_vanishes_mid_read_does_not_panic() {
        let d = FakeSys::new().read("gone");
        assert_eq!(d.hardware, None);
        assert_eq!(d.mac, None);
    }

    #[test]
    fn classifies_interfaces() {
        let k = |t: Traits| classify(&t);
        assert_eq!(
            k(Traits {
                name: "lo",
                arphrd: 772,
                ..Default::default()
            }),
            "Loopback"
        );
        assert_eq!(
            k(Traits {
                arphrd: 1,
                wireless: true,
                has_device: true,
                ..Default::default()
            }),
            "Wi-Fi"
        );
        assert_eq!(
            k(Traits {
                arphrd: 1,
                has_device: true,
                ..Default::default()
            }),
            "Ethernet"
        );
        assert_eq!(
            k(Traits {
                arphrd: 1,
                ..Default::default()
            }),
            "Virtual"
        );
        assert_eq!(
            k(Traits {
                arphrd: 1,
                devtype: "bridge",
                ..Default::default()
            }),
            "Bridge"
        );
        assert_eq!(
            k(Traits {
                arphrd: 65534,
                devtype: "wireguard",
                ..Default::default()
            }),
            "WireGuard VPN"
        );
        assert_eq!(
            k(Traits {
                name: "ztabc",
                arphrd: 1,
                tun_flags: Some(0x1002),
                ..Default::default()
            }),
            "ZeroTier VPN (TAP)"
        );
        assert_eq!(
            k(Traits {
                name: "tun0",
                arphrd: 65534,
                tun_flags: Some(0x1001),
                ..Default::default()
            }),
            "TUN tunnel"
        );
        assert_eq!(
            k(Traits {
                arphrd: 999,
                ..Default::default()
            }),
            "Link type 999"
        );
    }
}
