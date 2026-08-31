# localshare

Share a local HTTP server with the internet in one command.

## Usage

Run the relay server on a machine with a public IP or port access:

```bash
localshare serve
```

Expose a local server from another machine:

```bash
localshare client --target http://localhost:3000
```

## Status

Early stage, not yet production-ready.
