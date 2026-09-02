# localshare

![CI](https://github.com/unknownman/localshare/actions/workflows/ci.yml/badge.svg)
![Crates.io Version](https://img.shields.io/crates/v/localshare)
![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)

**Instantly share a local HTTP server with the internet. No accounts. No config. Beautiful terminal UX.**

`localshare` creates a WebSocket tunnel from a public URL to your local machine. Point it at a port, share the link, and your local server is accessible from anywhere — phones, tablets, other laptops, anywhere with a browser.

<!-- DEMO: Uncomment after recording with `vhs demo.tape` -->
<!-- ![localshare demo](demo.gif) -->

## Why localshare?

There are other tools that expose localhost to the internet. Here's how `localshare` compares:

| | **localshare** | **ngrok** | **localtunnel** | **bore** | **cloudflared** |
| --- | --- | --- | --- | --- | --- |
| **No account required** | ✅ | ❌ (mandatory) | ✅ | ✅ | ⚠️ (Cloudflare acct) |
| **No install beyond binary** | ✅ | ⚠️ (config files) | ❌ (needs Node.js) | ✅ | ⚠️ (config files) |
| **HTTP/HTTPS forwarding** | ✅ | ✅ | ✅ | ❌ (raw TCP) | ✅ |
| **Request logging in terminal** | ✅ | ✅ | ❌ | ❌ | ❌ |
| **QR code for mobile** | ✅ | ❌ | ❌ | ❌ | ❌ |
| **Self-hosted relay** | ✅ | ✅ (paid) | ❌ | ✅ | ❌ |
| **Written in Rust** | ✅ | ❌ (Go) | ❌ (JS) | ✅ | ❌ (Go) |

`localshare` is the zero-friction option: no auth tokens, no session limits, no Node.js dependency, and a polished terminal experience with colour-coded request logs and a scan-to-open QR code. If you need enterprise features like custom domains, OAuth, or webhook inspection, ngrok is the right tool. If you want the fastest possible raw TCP tunnel, bore is excellent. If you're already on Cloudflare and want tightly integrated DNS, cloudflared is a natural fit. For everything else — quick sharing, demos, testing webhooks on your phone, debugging an API on the local network — `localshare` gets out of your way.

## Quick start

```bash
cargo install localshare
```

Share a local server:

```bash
localshare 3000
```

That's it. `localshare` prints a public URL you can share immediately:

```
localshare 0.1.0 • Share your local server instantly
┌─────────────────────────────────────────────────────────────────┐
│ Public URL :  https://a1b2c3.relay.localshare.dev              │
│ Forwarding :  http://127.0.0.1:3000                            │
│ Status     :  ● LIVE  (relay.localshare.dev)                   │
└─────────────────────────────────────────────────────────────────┘

 ▀▄▀▄▀▄▀▄▀▄▀▄▀▄
 ▄▀▄▀▄▀▄▀▄▀▄▀▄▀▄    ← scan with your phone camera
 ▀▄▀▄▀▄▀▄▀▄▀▄▀▄
```

## Usage

```bash
# Share a local port (defaults to 127.0.0.1)
localshare 3000

# Share a specific host and port
localshare 127.0.0.1:8080

# Request a custom subdomain
localshare 5173 -s my-preview

# Suppress the QR code
localshare 3000 --no-qr

# Machine-readable JSON output for scripting
localshare 3000 --json

# Use a self-hosted relay
localshare 3000 -r relay.example.com
```

### Custom relay

By default `localshare` connects to the hosted relay at `relay.localshare.dev`. To use your own relay (or a tunnel server running on your infrastructure), pass its address with `-r`:

```bash
# Bare hostname — interpreted as ws://relay.example.com:80
localshare 3000 -r relay.example.com

# Host with explicit port
localshare 3000 -r relay.example.com:8080

# Explicit ws:// or wss:// URL (wss defaults to port 443)
localshare 3000 -r wss://relay.example.com
localshare 3000 -r ws://192.168.1.10:8080
```

The relay address can also be set via the `LOCALSHARE_RELAY` environment variable:

```bash
export LOCALSHARE_RELAY=ws://relay.internal:8080
localshare 3000
```

When using your own relay, the subdomain (`-s`), heartbeat, and reconnect behaviour are identical — no additional relay-side configuration is required.

### Options

| Flag | Description |
| --- | --- |
| `-r, --relay <URL>` | Relay server address (default: `relay.localshare.dev`, env: `LOCALSHARE_RELAY`) |
| `-s, --subdomain <NAME>` | Request a custom subdomain prefix |
| `--no-qr` | Suppress the terminal QR code |
| `--json` | Output tunnel metadata as JSON for scripting |
| `-q, --quiet` | Suppress non-error console output |
| `-v, --verbose` | Increase logging verbosity (`-v`, `-vv`, `-vvv`) |

### Pre-built binaries

Pre-built binaries for Linux (`x86_64`, `aarch64`), macOS (`x86_64`, `aarch64`), and Windows (`x86_64`) are attached to every [release](https://github.com/unknownman/localshare/releases). Download the archive for your platform, unpack it, and put the binary on your `PATH`.

## Features

- **Terminal QR code** — scan the public URL straight from your terminal to open it on a phone or tablet.
- **Zero-setup public URLs** — no sign-up, no auth tokens, no port forwarding. Every connection gets an instantly shareable link.
- **Graceful reconnections** — if the tunnel drops, `localshare` reconnects with exponential back-off and jitter, preserving your subdomain where possible. `SIGINT`/`SIGTERM` tear the tunnel down cleanly, flushing `Unregister` to the relay.
- **Live request log** — see each proxied request (method, path, status, latency) colour-coded in real time.
- **Actionable errors** — every failure mode (DNS, TLS, refused, relay full, subdomain taken…) gets a specific fix-it hint instead of a raw stack trace.
- **Self-hostable relay** — point `-r` at your own relay to keep everything on infrastructure you control.

## Limitations

Be aware of these before relying on `localshare` in production:

- **HTTP/1.1 only.** The forwarding engine handles HTTP/1.1 requests and responses. HTTP/2, WebSockets, and chunked transfer encoding are not yet supported.
- **Request bodies are buffered.** The entire request body is buffered before forwarding to the local server. Large uploads will increase latency.
- **Default relay is a convenience, not infrastructure.** The hosted relay at `relay.localshare.dev` is provided for zero-setup demos and development. It is not designed for sustained high-volume production traffic. If you need reliability at scale, run your own relay with `-r`.
- **No persistent custom domains.** In v0.1.0, subdomains are best-effort and not guaranteed to persist across relay restarts. There is no domain registration, DNS management, or automated TLS certificate provisioning for user domains.

## Release targets

`localshare` is compiled and tested on the following platforms:

| Target triple | OS | Arch |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | Linux | x86_64 |
| `aarch64-unknown-linux-gnu` | Linux | ARM64 |
| `x86_64-apple-darwin` | macOS | Intel |
| `aarch64-apple-darwin` | macOS | Apple Silicon |
| `x86_64-pc-windows-msvc` | Windows | x86_64 |

CI runs `cargo fmt`, `clippy`, and the full test suite on Linux, macOS, and Windows on every push. Release binaries are built automatically when a `v*` tag is pushed.

## License

[MIT](LICENSE)
