# Releasing Lyra

Tags use the bare version, e.g. `0.3.0` (no `v` prefix).

## 1. Bump the version

Update `rust/Cargo.toml` and let `Cargo.lock` update:

```sh
cd rust
cargo build --release
```

Commit the version bump, then tag and push:

```sh
git tag 0.3.0
git push origin main
git push origin 0.3.0
```

The `release` workflow builds the Linux x86_64 and Linux arm64 archives, writes `SHA256SUMS`, and
publishes the GitHub release.

## 2. macOS asset

GitHub-hosted macOS runners are not currently available to this private repository, so the
`build-macos` job is skipped unless the repository variable `ENABLE_MACOS_RUNNER=true` is set. Build
the Apple Silicon archive on the release host:

```sh
cargo build --release --locked --manifest-path rust/Cargo.toml
tmp=$(mktemp -d)
cp rust/target/release/lyra "$tmp/lyra"
tar -czf dist/lyra-aarch64-apple-darwin.tar.gz -C "$tmp" lyra
rm -rf "$tmp"
```

After the workflow publishes the release, upload the macOS archive and refresh the complete checksum
file:

```sh
mkdir -p dist/linux
gh release download 0.3.0 --dir dist/linux --pattern '*.tar.gz'
cp dist/lyra-aarch64-apple-darwin.tar.gz dist/linux/
(cd dist/linux && sha256sum lyra-*.tar.gz > SHA256SUMS)
gh release upload 0.3.0 dist/lyra-aarch64-apple-darwin.tar.gz --clobber
gh release upload 0.3.0 dist/linux/SHA256SUMS --clobber
```

## 3. Verify the public surface

```sh
curl -fsSL https://raw.githubusercontent.com/atom2ueki/lyra/main/install.sh | sh
lyra --version
lyra upgrade --check
```

The repository must be public for the unauthenticated `curl | sh` form. For a private repository,
use the authenticated form in `README.md` or set `LYRA_GITHUB_TOKEN` so the installer uses
`gh release download`.
