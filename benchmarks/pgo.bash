#!/usr/bin/env bash
# Build, train, and rebuild mbx with rustc profile-guided optimization.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
cd "$repo_root"

pgo_target="${MBX_PGO_TARGET:-}"
pgo_build_tool="${MBX_PGO_BUILD_TOOL:-cargo}"
pgo_profile="${MBX_PGO_PROFILE:-release}"
pgo_data_dir="$repo_root/target/pgo-data"
profraw_dir="$pgo_data_dir/profraw"
merged_profile="$pgo_data_dir/merged.profdata"
base_rustflags="${RUSTFLAGS:-}"

target_arg=""
target_dir_part=""
if [ -n "$pgo_target" ]; then
	target_arg="--target=$pgo_target"
	target_dir_part="$pgo_target/"
fi

rustc_host="$(rustc -vV | sed -n 's/^host: //p')"
rustc_sysroot="$(rustc --print sysroot)"
llvm_profdata="$rustc_sysroot/lib/rustlib/$rustc_host/bin/llvm-profdata"
if [ ! -x "$llvm_profdata" ]; then
	echo "llvm-profdata is missing; install rustup component llvm-tools" >&2
	exit 1
fi

mkdir -p "$profraw_dir"
rm -f "$profraw_dir"/*.profraw "$merged_profile"

# cross mounts the repository at another path. Make the host spelling of the
# merged profile visible there too, because rustc reads it during phase three.
if [ "$pgo_build_tool" = cross ]; then
	export CROSS_CONTAINER_OPTS="${CROSS_CONTAINER_OPTS:-} -v $pgo_data_dir:$pgo_data_dir:rw"
fi

echo ">>> [1/3] building instrumented mbx"
# shellcheck disable=SC2086 # An empty target_arg must disappear.
RUSTFLAGS="${base_rustflags:+$base_rustflags }-Cprofile-generate=$profraw_dir" \
	"$pgo_build_tool" build --profile "$pgo_profile" $target_arg --locked --package mbx

instrumented="$repo_root/target/${target_dir_part}${pgo_profile}/mbx"
if [ ! -x "$instrumented" ]; then
	echo "instrumented binary missing: $instrumented" >&2
	exit 1
fi

echo ">>> [2/3] training mbx"
export LLVM_PROFILE_FILE="$profraw_dir/mbx-%m-%p.profraw"
"$script_dir/train-pgo.bash" "$instrumented"
unset LLVM_PROFILE_FILE

profraw_count="$(find "$profraw_dir" -maxdepth 1 -name '*.profraw' -type f | wc -l | tr -d ' ')"
if [ "$profraw_count" -eq 0 ]; then
	echo "training produced no profile data" >&2
	exit 1
fi
echo ">>> collected $profraw_count raw profiles"

echo ">>> [3/3] merging profiles and rebuilding mbx"
"$llvm_profdata" merge -o "$merged_profile" "$profraw_dir"
test -s "$merged_profile"

# shellcheck disable=SC2086 # An empty target_arg must disappear.
RUSTFLAGS="${base_rustflags:+$base_rustflags }-Cprofile-use=$merged_profile -Cllvm-args=-pgo-warn-missing-function=false" \
	"$pgo_build_tool" build --profile "$pgo_profile" $target_arg --locked --package mbx

"$instrumented" --version
ls -lh "$instrumented"
