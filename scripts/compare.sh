#!/bin/sh
# Cross-language comparison: the programs in benchmarks/compare/, written the
# same way in Hiko, Lua and Python (see benchmarks/compare/README.md).
#
# Usage: scripts/compare.sh [output.md] [benchmark ...]
#   RUNS=n            runs per implementation (default 10)
#   HIKO_BIN=path     use this hiko-cli binary instead of building one
#   ALLOW_BATTERY=1   measure on battery power (macOS refuses otherwise)
#
# Needs hyperfine, lua and python3; uses luajit when installed. Every
# implementation of a program must print the same line, or the script fails.
set -eu

cd "$(dirname "$0")/.."
runs=${RUNS:-10}
out=${1:-target/compare.md}
[ $# -gt 0 ] && shift

for tool in hyperfine lua python3; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "compare.sh needs $tool" >&2
        exit 1
    fi
done

if [ -z "${HIKO_BIN:-}" ]; then
    cargo build --release -p hiko-cli
    HIKO_BIN=target/release/hiko-cli
fi
hiko="$HIKO_BIN run --config hiko.toml --policy benchmark"

on_battery() {
    command -v pmset >/dev/null 2>&1 && ! pmset -g batt | head -1 | grep -q "AC Power"
}

benchmarks=${*:-$(ls benchmarks/compare/*.hml | xargs -n1 basename | sed 's/\.hml$//')}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

{
    echo "# Cross-language comparison"
    echo
    echo "- date: $(date -u +%Y-%m-%d)"
    echo "- commit: $(git rev-parse --short HEAD)"
    echo "- machine: $(uname -sm), $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -p)"
    echo "- $(lua -v 2>&1 | head -1 | cut -d' ' -f1-2); $(python3 --version 2>&1)"
    if command -v luajit >/dev/null 2>&1; then
        echo "- $(luajit -v 2>&1 | head -1 | cut -d' ' -f1-2)"
    fi
    echo "- runs per implementation: $runs (hyperfine mean)"
    echo
    echo "| Program | Hiko | Lua | Python | LuaJIT | Hiko / Lua | Hiko / Python |"
    echo "|---|---|---|---|---|---|---|"
} >"$out"

for b in $benchmarks; do
    if on_battery && [ -z "${ALLOW_BATTERY:-}" ]; then
        echo "on battery power: plug in, or set ALLOW_BATTERY=1" >&2
        exit 2
    fi
    set -- "$hiko benchmarks/compare/$b.hml" "lua benchmarks/compare/$b.lua" \
        "python3 benchmarks/compare/$b.py"
    if command -v luajit >/dev/null 2>&1; then
        set -- "$@" "luajit benchmarks/compare/$b.lua"
    fi

    # Every implementation must print the same last line.
    expected=$($hiko "benchmarks/compare/$b.hml" 2>/dev/null | tail -1)
    for cmd in "$@"; do
        got=$($cmd 2>/dev/null | tail -1)
        if [ "$got" != "$expected" ]; then
            echo "$b: '$cmd' printed '$got', Hiko printed '$expected'" >&2
            exit 1
        fi
    done

    hyperfine -N --warmup 1 --runs "$runs" --export-json "$tmp/$b.json" "$@" >&2
    python3 - "$tmp/$b.json" "$b" >>"$out" <<'EOF'
import json, sys
results = json.load(open(sys.argv[1]))["results"]
means = [r["mean"] for r in results]
hiko, lua, py = means[0], means[1], means[2]
jit = f"{means[3] * 1000:.0f} ms" if len(means) > 3 else "-"
ms = lambda s: f"{s * 1000:.0f} ms"
print(f"| `{sys.argv[2]}` | {ms(hiko)} | {ms(lua)} | {ms(py)} | {jit} | {hiko / lua:.1f}x | {hiko / py:.1f}x |")
EOF
done

echo "wrote $out" >&2
