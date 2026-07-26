# Grav Tray RS

A lightweight native macOS menu bar app that shows quota availability for the Google Antigravity CLI (`agy`).

## Features

- Shows every enabled Gemini and Claude/GPT quota bucket
- Displays remaining quota, progress, and reset countdowns
- Lets you choose which quota appears in the menu bar
- Supports Launch at Login without requiring an `.app` bundle

## Requirements

- macOS 13 Ventura or later
- [Rust](https://rustup.rs/) 1.85 or later
- Antigravity CLI installed
- An open, authenticated `agy` session

## Install with Cargo

Install the release build from crates.io:

```sh
cargo install grav-tray-rs
grav-tray-rs --install
```

`--install` creates and starts a per-user LaunchAgent. Grav Tray then runs in the background and starts automatically when you sign in.

To run it only for the current terminal session:

```sh
grav-tray-rs
```

To stop it and disable Launch at Login:

```sh
grav-tray-rs --uninstall
```

If the menu cannot find `agy`, run the built-in connection check:

```sh
grav-tray-rs --diagnose
```

Run `grav-tray-rs --uninstall` before `cargo uninstall grav-tray-rs` so no
stale LaunchAgent remains.

## Build from source

```sh
git clone https://github.com/AABelkhiria/grav-tray-rs.git
cd grav-tray-rs
cargo test
cargo run --release
```

## How it works

An authenticated `agy` process exposes a quota RPC on localhost. Grav Tray:

1. Reads the beginning of recent Antigravity CLI logs to find active HTTP ports.
2. Calls the localhost-only `RetrieveUserQuotaSummary` RPC on a background thread.
3. Displays all quota groups using a native AppKit status item.
4. Reuses the working port and scans the logs again only when the session changes.

Authentication stays inside `agy`. Grav Tray does not read OAuth credentials or call the remote quota API directly.

Settings are stored at:

```text
~/Library/Application Support/grav-tray-rs/config.json
```

## License

[MIT](LICENSE)
