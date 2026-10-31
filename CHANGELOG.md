# Changelog

All notable changes are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1] - 2026-10-31

### Fixed

- Colors match btop's builtin Default theme, including when `color_theme` is `Default` or `TTY`, and theme files are resolved the way btop resolves them.
- A failed read of `/proc/net/dev` no longer spikes the rate and leaves that spike as the peak.
- The terminal is restored if startup fails after raw mode is enabled, and on `SIGHUP`, `SIGTERM` and Ctrl-Z.
- A Wi-Fi scan that fails is labeled "scan unavailable" rather than "hidden SSID".
- The `+N` address marker stays inside the box when ten or more addresses are left off.
- UNII-4 channels 181 and 185, and 6 GHz channel 2, get a band and channel. Wi-Fi 6E is only shown on 6 GHz.

### Changed

- Wi-Fi link details refresh on a background thread, so a scan cannot stall the display.

The fixes in this release were made by [Grok](https://x.ai), xAI's coding agent.

## [0.2.0] - 2026-10-31

### Added

- All IPv4 and IPv6 addresses with their prefix lengths, like `ip a`.
- ZeroTier network name, ID, status and type, using `zerotier-cli` or the daemon's network files.

### Changed

- Interface details are on fixed, labeled rows, so they line up across boxes and never wrap.
- All interface boxes are the same size, at most three across, with blank space for leftover slots.
- Paging replaces scrolling. Left and right arrows and `h` and `l` also change page.

## [0.1.0] - 2026-10-31

### Added

- A box per network interface, with an in/out history chart and a statistics table.
- An "all traffic" rollup with LED-style level meters and peak hold.
- Interface details: connection type, hardware name, driver, MAC, MTU and duplex.
- Wi-Fi details: SSID, signal, band, channel, width, generation and transmit rate.
- Colors from the active btop theme.
- Switching between bytes and bits, adjustable sampling and scrolling.

[Unreleased]: https://github.com/cpknight/nitop/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/cpknight/nitop/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/cpknight/nitop/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/cpknight/nitop/releases/tag/v0.1.0
