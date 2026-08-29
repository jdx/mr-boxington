#!/usr/bin/env bash
# Hermetic training workload shared by rustc PGO and post-link optimization.

set -euo pipefail

if [ "$#" -ne 1 ] || [ ! -x "$1" ]; then
	echo "usage: $0 /path/to/mbx" >&2
	exit 2
fi

mbx="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
train_dir="$(mktemp -d "${TMPDIR:-/tmp}/mbx-pgo.XXXXXX")"
cleanup() {
	case "$train_dir" in
	"${TMPDIR:-/tmp}"/mbx-pgo.*) rm -rf "$train_dir" ;;
	*) echo "refusing to remove unexpected training directory: $train_dir" >&2 ;;
	esac
}
trap cleanup EXIT

project="$train_dir/project"
mkdir -p "$project/src"

cat >"$project/Cargo.toml" <<'EOF'
[package]
name = "mbx-pgo-subject"
version = "0.0.0"
edition = "2024"
build = "build.rs"

[[bin]]
name = "mbx-pgo-subject"
path = "src/main.rs"
EOF

cat >"$project/src/lib.rs" <<'EOF'
pub fn digest(seed: u64) -> u64 {
    (0..4096).fold(seed, |value, byte| value.rotate_left(7) ^ byte)
}
EOF

cat >"$project/src/main.rs" <<'EOF'
fn main() {
    println!("{}", mbx_pgo_subject::digest(42));
}
EOF

cat >"$project/build.rs" <<'EOF'
use std::{env, process::Command};

fn main() {
    let compiler = env::var_os("CC").unwrap_or_else(|| "cc".into());
    let output = std::path::PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("native.o");
    let status = Command::new(compiler)
        .args(["-c", "native.c", "-o"])
        .arg(output)
        .status()
        .unwrap();
    assert!(status.success());
    println!("cargo:rerun-if-changed=native.c");
}
EOF

cat >"$project/native.c" <<'EOF'
unsigned long mbx_pgo_native(unsigned long value) {
    return (value << 5) ^ (value >> 3);
}
EOF

export MBX_CACHE_DIR="$train_dir/cache"
export MBX_TARGET_VIEWS=0
export CARGO_TARGET_DIR="$train_dir/target"

# Weight short-lived dispatch because Cargo reaches the rustc and cc shims far
# more often than a person reaches a subcommand.
for _ in $(seq 1 30); do
	"$mbx" --help >/dev/null
	"$mbx" --version >/dev/null
done

# First-touch compilation trains misses and publication. Removing only the
# throwaway target then trains prediction lookup, blob reads, decompression,
# and output materialization from a warm action store.
(
	cd "$project"
	"$mbx" build --offline
	for _ in $(seq 1 8); do
		rm -rf "$CARGO_TARGET_DIR"
		"$mbx" build --offline
	done
	"$mbx" check --offline
	"$mbx" cache stats >/dev/null
	"$mbx" gc --dry-run >/dev/null
)
