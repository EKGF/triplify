#!/usr/bin/env bash
# Benchmark triplify on generated inputs: wall time, peak RSS, quads/second.
#
#   scripts/bench.sh [scale] [case...]
#
# Inputs are generated once per scale under target/bench-data/<scale>.
# Never uses `set -e`; every command's status is checked.
set -uo pipefail

scale="${1:-1.0}"
shift || true
cases=("$@")
if (( ${#cases[@]} == 0 )); then
    cases=(csv csv-gz csv-profile xlsx parquet parquet-profile many)
fi

root="$(cd "$(dirname "$0")/.." && pwd)" || exit 1
data="$root/target/bench-data/$scale"
out="$root/target/bench-out"
mkdir -p "$out" || exit 1

cargo build --release --locked --bins --manifest-path "$root/Cargo.toml" || {
    printf 'release build failed\n' >&2; exit 1; }
bin="$(cargo metadata --format-version 1 --no-deps --manifest-path "$root/Cargo.toml" \
    | python3 -c 'import sys,json; print(json.load(sys.stdin)["target_directory"])')/release/triplify" || exit 1

if [[ ! -f "$data/table.parquet" ]]; then
    cargo run --release --locked --example bench_data --manifest-path "$root/Cargo.toml" -- "$data" "$scale" || {
        printf 'data generation failed\n' >&2; exit 1; }
fi

run() {
    local name="$1"; shift
    local log="$out/$name.log" timing="$out/$name.time"
    if ! /usr/bin/time -l "$bin" -v "$@" 2>"$log" >/dev/null; then
        printf '%-16s FAILED (see %s)\n' "$name" "$log" >&2
        return 1
    fi
    # /usr/bin/time -l writes to the same stderr; split it out.
    grep -E 'real|maximum resident' "$log" > "$timing" || true
    local wall rss quads
    wall=$(awk '/real/ {print $1}' "$timing")
    rss=$(awk '/maximum resident/ {printf "%.0f", $1/1048576}' "$timing")
    quads=$(grep -oE 'quads: [0-9]+' "$log" | grep -oE '[0-9]+' | tail -1)
    printf '%-16s %8.2f s %7s MiB %12s quads %10.0f quads/s\n' \
        "$name" "$wall" "$rss" "${quads:-?}" "$(python3 -c "print(${quads:-0}/max($wall,1e-9))")"
}

status=0
for c in "${cases[@]}"; do
    case "$c" in
        csv)             run csv "$data/wide.csv" || status=1 ;;
        csv-gz)          run csv-gz "$data/wide.csv" -o "$out/wide.nq.gz" || status=1 ;;
        csv-profile)     run csv-profile --profile "$data/wide.csv" || status=1 ;;
        xlsx)            run xlsx "$data/book.xlsx" || status=1 ;;
        parquet)         run parquet "$data/table.parquet" || status=1 ;;
        parquet-profile) run parquet-profile --profile "$data/table.parquet" || status=1 ;;
        many)            run many "$data/many" || status=1 ;;
        *) printf 'unknown case %s\n' "$c" >&2; status=1 ;;
    esac
done
exit "$status"
