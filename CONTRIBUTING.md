# Contributing to nitop

Thanks for helping out. nitop is a small, focused tool: one screen showing what your network interfaces are doing. Changes that keep it that way are the most likely to be accepted, so for anything large, please open an issue to discuss it first.

## Development

You need Rust 1.89 or newer, on Linux.

```sh
cargo run                 # run it
cargo test                # run all tests
cargo fmt                 # format
cargo clippy --all-targets -- -D warnings
```

CI runs the format check, clippy and tests on every pull request, so running them first saves a round trip.

## Layout

| File | What it does |
| --- | --- |
| `src/main.rs` | Arguments, key handling, event loop |
| `src/net.rs` | Reads kernel counters and derives rates and totals |
| `src/ifinfo.rs` | Works out what an interface is, from sysfs and `hwdata` |
| `src/wifi.rs` | Wi-Fi link state over nl80211, refreshed on a background thread |
| `src/zerotier.rs` | ZeroTier network details, on a background thread |
| `src/meter.rs` | Level meter behavior and scale |
| `src/theme.rs` | Loads colors from the btop theme |
| `src/ui.rs` | All drawing |
| `src/screenshot.rs` | Test-only: renders the README screenshot |
| `tests/cli.rs` | Runs the real binary, including in a pseudo-terminal |

## Testing

New behavior should come with tests. Logic is kept separate from the system so it can be tested without special hardware: `ifinfo.rs` tests build a fake sysfs tree, and `wifi.rs` tests decode hand-built netlink messages. If you add support for a new kind of interface, add a case there.

## Screenshot

`docs/screenshot.png` is rendered from made-up data by the real drawing code. **Don't replace it with a capture of your own machine,** because that would publish your SSID, addresses and MAC.

```sh
cargo test generate_screenshot -- --ignored
rsvg-convert -z 2 target/screenshot.svg -o docs/screenshot.png
```

## Pull requests

- Keep each pull request to one change.
- Add user-visible changes to `CHANGELOG.md`, under "Unreleased".
- Update the README if behavior changes.

AI-assisted contributions are welcome, as nitop itself was written that way. Please make sure you've read and tested what you submit.

## Releasing (maintainers)

1. Update `version` in `Cargo.toml`, run `cargo check` to update `Cargo.lock`, and move "Unreleased" in `CHANGELOG.md` to a dated version.
2. Commit, then tag and push, using the new version: `git tag vX.Y.Z && git push origin main vX.Y.Z`.
3. Wait for the release workflow. It checks the tag against `Cargo.toml`, builds the binaries and publishes the GitHub release.
4. If nitop is on crates.io, run `cargo publish` from a clean checkout of the tag. A published version can't be changed or reused.
5. If nitop is on the AUR, update `nitop` and `nitop-bin`: set `pkgver`, reset `pkgrel` to 1, run `updpkgsums`, regenerate `.SRCINFO` with `makepkg --printsrcinfo > .SRCINFO`, and push. `nitop-bin` needs the files from step 3.
