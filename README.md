# nitop

[![CI](https://github.com/cpknight/nitop/actions/workflows/ci.yml/badge.svg)](https://github.com/cpknight/nitop/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/cpknight/nitop)](https://github.com/cpknight/nitop/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A [btop](https://github.com/aristocratos/btop)-style terminal monitor for network interfaces. It shows every interface at a glance, each in its own box, with one rollup of all your traffic on top.

![nitop showing an all-traffic rollup with level meters above boxes for Ethernet, Wi-Fi, WireGuard and loopback interfaces](docs/screenshot.png)

*The screenshot uses made-up data.*

## Features

- **A box per interface**, each with an in/out braille history chart and a table of rate, peak, average, session and kernel totals, packets, errors and drops.
- **An "all traffic" rollup** with stereo-style LED level meters: instant level plus a peak-hold marker.
- **What each interface is:** connection type, hardware name, driver, MAC, MTU and duplex. On Wi-Fi it adds SSID, signal, band, channel, width, generation and transmit rate.
- **Uses your btop theme** automatically, if you have one.
- **Light and self-contained:** a single binary with no runtime dependencies, no root and no external tools.
- **Follows your system:** adapts to the terminal size, and picks up interfaces as they appear and disappear (VPNs, containers, USB adapters).

nitop shows traffic per interface. For per-connection or per-process traffic, use a tool like `iftop`, `nethogs` or `bandwhich`.

## Install

nitop runs on Linux only. It works on x86-64 and ARM64.

**Prebuilt binary:**

```sh
curl -L https://github.com/cpknight/nitop/releases/latest/download/nitop-x86_64-unknown-linux-musl.tar.gz | tar xz nitop
install -Dm755 nitop ~/.local/bin/nitop
```

For ARM64, use `nitop-aarch64-unknown-linux-musl.tar.gz`. Checksums are on the [releases page](https://github.com/cpknight/nitop/releases).

**From source** (requires Rust 1.89 or newer):

```sh
cargo install --git https://github.com/cpknight/nitop
```

## Usage

Run `nitop`. It has no options besides `--help` and `--version`.

| Key | Action |
| --- | --- |
| `q`, `Esc`, `Ctrl-C` | Quit |
| `b` | Show rates in bytes or bits |
| `+` / `-` | Slower / faster sampling (100 ms to 10 s) |
| `↑` `↓`, `j` `k`, `PgUp` `PgDn`, `Home` `End` | Scroll interface boxes when they don't all fit |

## Reading the display

- **Charts:** inbound grows up from the middle and outbound grows down. Each has its own scale, shown in the corner, so a small upload stays visible beside a large download. That means their heights can't be compared directly.
- **Level meters:** a fixed log scale from 1K to 10G, so a position always means the same rate. The bar jumps up instantly and falls smoothly. A single lit segment marks the recent peak, holding about 2.5 s before dropping. Readings arrive at the sampling interval, and the movement between them is animation.
- **Session, Peak and Avg** are measured since nitop started. **Total, Errors and Drops** are the kernel's counters.
- **The rollup** sums every interface except loopback. Tunnel traffic (VPN, ZeroTier) is counted on the tunnel and again on the physical link that carries it, so the rollup reads high while a tunnel is busy.
- **Details** that don't fit in a box are dropped, with the most useful kept.

## Good to know

- **Hardware names** come from the `hwdata` package's `pci.ids` and `usb.ids`. Without it, or for a card it doesn't list, you see the numeric ID or a more general chip name.
- **VPN product names** (ZeroTier, Tailscale and so on) are inferred from the interface name for TUN/TAP devices, because the kernel doesn't record who owns one.
- **Theme:** read at startup from `color_theme` in `~/.config/btop/btop.conf`, with btop-like defaults otherwise. Restart nitop to pick up a theme change.

## Contributing

Bug reports and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md). Hardware reports are especially useful: if an interface is labeled wrongly, please [open an issue](https://github.com/cpknight/nitop/issues/new/choose).

## Credits

nitop was written by [Claude](https://claude.com/claude-code), Anthropic's AI coding agent, working from the direction of its maintainer, [@cpknight](https://github.com/cpknight). The look comes from [btop](https://github.com/aristocratos/btop), and the terminal UI is built with [ratatui](https://ratatui.rs).

## License

[MIT](LICENSE)
