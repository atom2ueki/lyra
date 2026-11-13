#!/bin/sh
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)
bin="${LYRA_BIN:-$root/rust/target/debug/lyra}"
if [ ! -x "$bin" ]; then
  cargo build --quiet --locked --manifest-path "$root/rust/Cargo.toml"
fi
LYRA_BIN="$bin" node "$here/runner.mjs"
