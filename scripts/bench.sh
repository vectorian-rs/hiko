#!/bin/sh
# Performance baseline for Hiko. Results and how to read them are in
# docs/architecture/performance.md.
#
# Usage: scripts/bench.sh [output.md]
#   RUNS=n       runs per benchmark (default 10)
#   HIKO_BIN=p   use this hiko-cli binary instead of building one
#
# Needs hyperfine. Run from a clean checkout on an idle machine.
set -eu

cd "$(dirname "$0")/.."
runs=${RUNS:-10}
out=${1:-target/bench.md}

if ! command -v hyperfine >/dev/null 2>&1; then
    echo "bench.sh needs hyperfine (https://github.com/sharkdp/hyperfine)" >&2
    exit 1
fi

if [ -z "${HIKO_BIN:-}" ]; then
    cargo build --release -p hiko-cli
    HIKO_BIN=target/release/hiko-cli
fi
hiko="$HIKO_BIN run --config hiko.toml --policy benchmark"

mkdir -p "$(dirname "$out")"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

{
    echo "# Hiko benchmarks"
    echo
    echo "- date: $(date -u +%Y-%m-%d)"
    echo "- commit: $(git rev-parse --short HEAD)"
    echo "- machine: $(uname -sm), $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -p)"
    echo "- runs per benchmark: $runs"
    echo
    echo "## Process creation"
    echo
    echo '```text'
    cargo run -q --release -p hiko-vm --example process_creation_cost
    echo '```'
} >"$out"

bench() {
    title=$1
    shift
    hyperfine -N --warmup 2 --runs "$runs" --export-markdown "$tmp/part.md" "$@" >&2
    {
        echo
        echo "## $title"
        echo
        cat "$tmp/part.md"
    } >>"$out"
}

# Single-process interpreter benchmarks.
set --
for f in benchmarks/ack.hml benchmarks/tak.hml benchmarks/nqueens.hml \
    benchmarks/primes.hml benchmarks/ec_ack.hml benchmarks/ec_fib.hml \
    benchmarks/early_exit.hml; do
    set -- "$@" "$hiko $f"
done
bench "Interpreter" "$@"

bench "Spawn and await 6,000 children" \
    -L workers 1,8 "$hiko --workers {workers} benchmarks/spawn_await.hml"

bench "One list shared by 200 children" "$hiko benchmarks/spawn_shared.hml"

bench "CPU fan-out, 64 children" \
    -L workers 1,2,4,8 "$hiko --workers {workers} benchmarks/fanout_cpu.hml"

bench "File-read fan-out, 1,000 children" \
    -L workers 1,2,4,8 "$hiko --workers {workers} benchmarks/fanout_read.hml"

# examples/benchmark.hml walks .git. In a git worktree .git is a file, so
# walk a copy of the repository's git directory instead. It needs every
# filesystem builtin, so it runs under the default policy.
git_dir=.git
if [ ! -d .git ]; then
    git_dir=target/bench-git
    [ -d "$git_dir" ] || cp -R "$(git rev-parse --git-common-dir)" "$git_dir"
fi
sed "s|\"\\.git\"|\"$git_dir\"|" examples/benchmark.hml >target/bench-walk.hml
sed "s|\"\\.git\"|\"$git_dir\"|" examples/benchmark.py >target/bench-walk.py
if python3 -c "import blake3" 2>/dev/null; then
    bench "Walk and hash $git_dir (vs Python)" \
        "$HIKO_BIN run --config hiko.toml target/bench-walk.hml" "python3 target/bench-walk.py"
else
    echo "skipping the walk benchmark: python3 has no blake3 module" >&2
fi

echo "wrote $out" >&2
