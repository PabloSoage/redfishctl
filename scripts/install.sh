#!/usr/bin/env sh
# Build redfishctl and install the binary on the PATH.
#
#   sh scripts/install.sh
#
# Uses `cargo install`, which drops the executable in ~/.cargo/bin. That
# directory is already on the PATH if Rust was installed through rustup, so
# afterwards `redfishctl` works from anywhere.

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

printf '\nBuilding and installing redfishctl...\n\n'
cargo install --path "$ROOT" --force

DEST="${CARGO_HOME:-$HOME/.cargo}/bin"
printf '\nInstalled in %s\n' "$DEST"

case ":$PATH:" in
    *":$DEST:"*)
        printf 'Run it with: redfishctl\n\n'
        ;;
    *)
        printf '\nThat directory is not on your PATH. Add this to ~/.bashrc or ~/.zshrc:\n\n'
        printf '  export PATH="%s:$PATH"\n\n' "$DEST"
        ;;
esac
