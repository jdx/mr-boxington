#!/usr/bin/env bash
# Apply profile-guided LLVM BOLT optimizations to a linked mbx ELF binary.

set -euo pipefail

if [ "$#" -ne 1 ] || [ ! -x "$1" ]; then
	echo "usage: $0 /path/to/mbx" >&2
	exit 2
fi

binary="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
llvm_bolt="${LLVM_BOLT:-llvm-bolt}"
merge_fdata="${MERGE_FDATA:-merge-fdata}"

for tool in "$llvm_bolt" "$merge_fdata"; do
	if ! command -v "$tool" >/dev/null 2>&1; then
		echo "required BOLT tool not found: $tool" >&2
		exit 1
	fi
done

llvm_bolt_path="$(readlink -f "$(command -v "$llvm_bolt")")"
llvm_bolt_prefix="$(cd "$(dirname "$llvm_bolt_path")/.." && pwd)"
runtime_lib="${BOLT_RUNTIME_INSTRUMENTATION_LIB:-$llvm_bolt_prefix/lib/libbolt_rt_instr.a}"
if [ ! -f "$runtime_lib" ]; then
	echo "BOLT instrumentation runtime not found: $runtime_lib" >&2
	exit 1
fi

# Debian's BOLT resolves this option relative to /usr/lib even when given an
# absolute path, otherwise producing /usr/lib/usr/lib/llvm-N/lib/...
runtime_arg="$runtime_lib"
case "$runtime_arg" in
/usr/lib/*) runtime_arg="${runtime_arg#/usr/lib/}" ;;
esac

if ! readelf -S "$binary" | grep -q '\.rela\.text'; then
	echo "$binary has no .rela.text section; link with --emit-relocs" >&2
	exit 1
fi

bolt_dir="$(mktemp -d "${TMPDIR:-/tmp}/mbx-bolt.XXXXXX")"
instrumented="$bolt_dir/mbx.instrumented"
profile_prefix="$bolt_dir/mbx.fdata"
merged_profile="$bolt_dir/merged.fdata"
optimized="$binary.bolt"
cleanup() {
	case "$bolt_dir" in
	"${TMPDIR:-/tmp}"/mbx-bolt.*) rm -rf "$bolt_dir" ;;
	*) echo "refusing to remove unexpected BOLT directory: $bolt_dir" >&2 ;;
	esac
	rm -f "$optimized"
}
trap cleanup EXIT

echo ">>> [1/3] instrumenting PGO-optimized binary with BOLT"
"$llvm_bolt" "$binary" \
	-instrument \
	-runtime-instrumentation-lib="$runtime_arg" \
	-instrumentation-file="$profile_prefix" \
	-instrumentation-file-append-pid \
	-o "$instrumented"

echo ">>> [2/3] training BOLT against the hermetic workload"
"$script_dir/train-pgo.bash" "$instrumented"

profile_count="$(find "$bolt_dir" -maxdepth 1 -name 'mbx.fdata.*' -type f | wc -l | tr -d ' ')"
if [ "$profile_count" -eq 0 ]; then
	echo "BOLT training produced no profiles in $bolt_dir" >&2
	exit 1
fi
echo ">>> collected $profile_count BOLT profiles"

# BOLT appends only numeric process IDs to this controlled prefix.
# shellcheck disable=SC2086
"$merge_fdata" "$profile_prefix".* >"$merged_profile"
test -s "$merged_profile"

echo ">>> [3/3] reordering and splitting hot code with BOLT"
"$llvm_bolt" "$binary" \
	-o "$optimized" \
	-data="$merged_profile" \
	-reorder-blocks=ext-tsp \
	-reorder-functions=cdsort \
	-split-functions \
	-split-all-cold \
	-split-eh \
	-use-gnu-stack \
	-dyno-stats

strip --strip-all "$optimized"
"$optimized" --version >/dev/null
mv "$optimized" "$binary"
echo ">>> BOLT optimization complete: $binary"
ls -lh "$binary"
