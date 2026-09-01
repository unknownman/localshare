# localshare

**Share a local HTTP server with the internet in one command.** No accounts, no configuration, no firewall juggling — just point localshare at your local port and get a public URL you can share instantly.

## Installation

Localshare is built with Rust. The easiest way to install it is via `cargo`:

```bash
cargo install localshare
```

## Usage

The interface is deliberately minimal — give localshare a target and it handles the rest.

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

### Options

| Flag | Description |
| --- | --- |
| `-r, --relay <URL>` | Relay server address (defaults to `relay.localshare.dev`) |
| `-s, --subdomain <NAME>` | Request a custom subdomain |
| `--no-qr` | Suppress the terminal QR code |
| `--json` | Output tunnel metadata as JSON for scripting |
| `-q, --quiet` | Suppress non-error console output |
| `-v, --verbose` | Increase logging verbosity (`-v`, `-vv`, `-vvv`) |

## Features

- **Terminal QR code** — scan the public URL straight from your terminal to open it on a phone or tablet.
- **Zero-setup public URLs** — no sign-up, no auth tokens, no port forwarding. Every connection gets an instantly shareable link.
- **Graceful reconnections** — if the tunnel drops, localshare automatically reconnects with exponential back-off and jitter, preserving your subdomain where possible.
- **Live request log** — see each proxied request (method, path, status, latency) colour-coded in real time.
- **Self-hostable relay** — point `-r` at your own relay to keep everything on infrastructure you control.

## Acknowledgements

Share your local server with the world, one command at a time.

## License

MIT
