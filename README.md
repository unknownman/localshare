# localshare

![CI](https://github.com/unknownman/localshare/actions/workflows/ci.yml/badge.svg)
![Crates.io Version](https://img.shields.io/crates/v/localshare)
![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)

**Share a local HTTP server with the internet in one command.** No accounts, no configuration, no firewall juggling — just point `localshare` at your local port and get a public URL you can share instantly.

## Installation

### From source (cargo)

`localshare` is built with Rust. If you have a stable Rust toolchain:

```bash
cargo install localshare
```

### Pre-built binaries

Pre-built binaries for Linux (`x86_64`, `aarch64`), macOS (`x86_64`, `aarch64`), and Windows (`x86_64`) are attached to every [release](https://github.com/unknownman/localshare/releases). Download the archive for your platform, unpack it, and put the `localshare` binary on your `PATH`:

```bash
# Linux/macOS
curl -LO "https://github.com/unknownman/localshare/releases/latest/download/localshare-$(uname -s)-$(uname -m).tar.gz"
tar -xzf localshare-*.tar.gz
sudo mv localshare /usr/local/bin/

# Windows (PowerShell)
# Download localshare-x86_64-pc-windows-msvc.zip, then:
# Expand-Archive localshare-x86_64-pc-windows-msvc.zip -DestinationPath .
# Move-Item localshare.exe $env:LOCALAPPDATA\Microsoft\WindowsApps\
```

> The `latest/download` URL pattern works only for the tagged release names produced by the [release workflow](.github/workflows/release.yml) (e.g. `v1.0.0`).

## Usage

The interface is deliberately minimal — give `localshare` a target and it handles the rest.

```bash
# Share a local port (defaults to localhost)
localshare 3000

# Share a specific host and port
localshare 127.0.0.1:8080

# Request a custom subdomain
localshare 5173 -s my-app

# Machine-readable output for scripting
localshare 3000 --json
```

Each run prints a friendly banner, your unique public URL, and a QR code you can scan to open the tunnel on your phone.

```
┌───────────────────────────────────────────────────────┐
│ Public URL :  https://a1b2c3.relay.localshare.dev      │
│ Forwarding :  http://127.0.0.1:3000                    │
│ Status     :  ● LIVE  (relay.localshare.dev)           │
└───────────────────────────────────────────────────────┘
```

### Custom relay

By default `localshare` uses the hosted relay at `relay.localshare.dev`. To use a relay you control (or a tunnel server running elsewhere), pass its address with `-r`:

```bash
# Bare hostname — interpreted as ws://relay.example.com:80
localshare 3000 -r relay.example.com

# Host with explicit port
localshare 3000 -r relay.example.com:8080

# Explicit ws:/wss: URL (wss defaults to port 443 when no port is given)
localshare 3000 -r wss://relay.example.com
localshare 3000 -r ws://192.168.1.10:8080
```

The relay address may also be supplied through the `LOCALSHARE_RELAY` environment variable, which can be handy for CI:

```bash
export LOCALSHARE_RELAY=ws://relay.internal:8080
localshare 3000
```

> When used with your own relay, the subdomain (`-s`), heartbeat, and reconnect behaviour are identical — everything below the relay is the client's responsibility, so a self-hosted relay needs no extra configuration on your machine.

### Options

| Flag | Description |
| --- | --- |
| `-r, --relay <URL>` | Relay server address (defaults to `relay.localshare.dev`, env `LOCALSHARE_RELAY`) |
| `-s, --subdomain <NAME>` | Request a custom subdomain |
| `--no-qr` | Suppress the terminal QR code |
| `--json` | Output tunnel metadata as JSON for scripting |
| `-q, --quiet` | Suppress non-error console output |
| `-v, --verbose` | Increase logging verbosity (`-v`, `-vv`, `-vvv`) |

## Features

- **Terminal QR code** — scan the public URL straight from your terminal to open it on a phone or tablet.
- **Zero-setup public URLs** — no sign-up, no auth tokens, no port forwarding. Every connection gets an instantly shareable link.
- **Graceful reconnections** — if the tunnel drops, `localshare` automatically reconnects with exponential back-off and jitter, preserving your subdomain where possible. `SIGINT`/`SIGTERM` (Ctrl+C on Windows) tear the tunnel down cleanly, flushing `Unregister` to the relay.
- **Live request log** — see each proxied request (method, path, status, latency) colour-coded in real time.
- **Actionable errors** — every failure mode (DNS, TLS, refused, relay full, subdomain taken…) gets a specific fix-it hint instead of a raw stack trace.
- **Self-hostable relay** — point `-r` at your own relay to keep everything on infrastructure you control.

## Acknowledgements

Share your local server with the world, one command at a time.

## License

[MIT](LICENSE)