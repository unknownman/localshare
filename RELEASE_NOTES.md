# Release Notes — localshare v0.1.1

## Installing

### From crates.io

```bash
cargo install localshare
```

### From source

```bash
git clone https://github.com/unknownman/localshare.git
cd localshare
cargo install --path .
```

### Pre-built binaries

Download the archive for your platform from the [GitHub Releases](https://github.com/unknownman/localshare/releases) page. Archives are named by target triple:

| Archive | Platform |
| --- | --- |
| `localshare-x86_64-unknown-linux-gnu.tar.gz` | Linux x86_64 |
| `localshare-aarch64-unknown-linux-gnu.tar.gz` | Linux ARM64 |
| `localshare-x86_64-apple-darwin.tar.gz` | macOS Intel |
| `localshare-aarch64-apple-darwin.tar.gz` | macOS Apple Silicon |
| `localshare-x86_64-pc-windows-msvc.zip` | Windows x86_64 |

## Releasing

### Tagging a release

```bash
git tag v0.1.1
git push origin v0.1.1
```

Pushing a `v*` tag triggers the [release workflow](.github/workflows/release.yml), which builds binaries for all target triples, packages them as `.tar.gz` (Unix) or `.zip` (Windows), and attaches them to the GitHub Release.

### Target triples

The release matrix compiles for:

| Triple | Runner | Cross-compiled? |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | No |
| `aarch64-unknown-linux-gnu` | `ubuntu-latest` | Yes (via `cross`) |
| `x86_64-apple-darwin` | `macos-13` | No |
| `aarch64-apple-darwin` | `macos-14` | No |
| `x86_64-pc-windows-msvc` | `windows-latest` | No |

### CI checks

Before tagging, ensure the CI pipeline passes:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

The CI workflow (`.github/workflows/ci.yml`) runs these checks on every push to `main`/`master` and on every pull request, across Linux, macOS, and Windows.

### crates.io publishing

When ready to publish to crates.io:

```bash
cargo login <your-token>
cargo publish
```

Ensure `Cargo.toml` has the correct `version`, `description`, `license`, `repository`, `keywords`, and `categories` fields before publishing. The `--allow-dirty` flag should not be used; commit all changes first.

### Generating a demo GIF

The `demo.tape` file records a terminal demo using [charmbracelet/vhs](https://github.com/charmbracelet/vhs):

```bash
vhs demo.tape
```

This produces `demo.gif`. Update the placeholder in `README.md` with the path to the generated file, or commit it to the repository and reference the raw GitHub URL.
