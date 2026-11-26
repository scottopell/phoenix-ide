#!/usr/bin/env bash
set -euo pipefail
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
probe_dir=$PWD/decoder-build/target
baseline=$probe_dir/baseline/release/decode-probe
candidate=$probe_dir/candidate/release/decode-probe
hex_only=$probe_dir/hex-only/release/decode-probe
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
export GIT_AUTHOR_NAME=Fixture GIT_AUTHOR_EMAIL=fixture@example.test
export GIT_COMMITTER_NAME=Fixture GIT_COMMITTER_EMAIL=fixture@example.test
cd "$scratch"
compare() {
    local name=$1 expected=$2 a=0 b=0 h=0
    shift 2
    "$baseline" "$@" < bytes > base-roots 2> base-error || a=$?
    "$candidate" "$@" < bytes > gix-roots 2> gix-error || b=$?
    "$hex_only" "$@" < bytes > hex-roots 2> hex-error || h=$?
    test "$a" -eq "$h"
    cmp base-roots hex-roots
    if [[ $a -eq 0 && $b -eq 0 ]]; then
        if cmp -s base-roots gix-roots; then result=accept; else result=different-roots; fi
    elif [[ $a -eq 1 && $b -eq 1 ]]; then result=reject
    elif [[ $a -eq 0 && $b -eq 1 ]]; then result=gix-rejects
    elif [[ $a -eq 1 && $b -eq 0 ]]; then result=gix-accepts
    else result=unexpected-status; fi
    printf '%s: %s (CLI-parser=%s gix=%s)\n' "$name" "$result" "$a" "$b"
    [[ $result != unexpected-status ]]
    [[ $result == "$expected" || $expected == observe ]]
}
actor='Fixture <fixture@example.test> 1600000000 +0000'
for format in sha1 sha256; do
    git init -qb main --object-format="$format" "$format"
    cd "$format"
    git commit -qm preserved --allow-empty
    seed=$(git rev-parse HEAD)
    git worktree add -q --detach "../worktree-$format"
    cd "../worktree-$format"
    git commit -qm old --allow-empty
    old=$(git rev-parse HEAD)
    git switch -q --detach "$seed"
    git commit -qm new --allow-empty
    new=$(git rev-parse HEAD)
    target=$(git rev-parse --git-path logs/HEAD)
    "$probe_dir/release/capture" "$target" > bytes
    ln -s "$target" symlink
    mkfifo fifo
    for unsafe_path in symlink fifo; do
        if "$probe_dir/release/capture" "$unsafe_path" > unsafe-bytes 2> unsafe-error; then
            echo "capture unexpectedly accepted $unsafe_path" >&2
            exit 1
        fi
        test ! -s unsafe-bytes
    done
    compare "$format real linked-worktree capture" accept
    zero=${seed//?/0}
    printf '\n%s %s %s\told\n\n%s %s %s\tnew' "$old" "$zero" "$actor" "$zero" "$new" "$actor" > bytes
    compare "$format blank + unterminated + both endpoints" accept
    printf '%s\n' "$old" "$new" | LC_ALL=C sort > expected-roots
    cmp expected-roots base-roots
    head=$seed
    for source in base gix; do
        mapfile -t roots < "$source-roots"
        git rev-list "$head" "${roots[@]}" --not --branches --remotes --tags > "$source-inventory"
    done
    cmp base-inventory gix-inventory
    grep -Fxq "$old" base-inventory
    grep -Fxq "$new" base-inventory
    git update-ref refs/stash "$old"
    for source in base gix; do
        mapfile -t roots < "$source-roots"
        git rev-list "$head" "${roots[@]}" --not --branches --remotes --tags refs/stash > "$source-inventory"
    done
    cmp base-inventory gix-inventory
    test "$(cat base-inventory)" = "$new"
    printf '%s %s \n' "$old" "$new" > bytes
    compare "$format empty actor" gix-rejects
    printf '%s %s opaque tail\n' "$old" "$new" > bytes
    compare "$format opaque actor" gix-rejects
    printf '%s %s %s\t\377\r\n' "$old" "$new" "$actor" > bytes
    compare "$format binary message + CRLF" observe
    printf '%s %s\n' "$old" "$new" > bytes
    compare "$format missing delimiter" reject
    printf '%s %s %s\n' "${old^^}" "$zero" "$actor" > bytes
    compare "$format uppercase endpoint" observe
    letters=$(printf '%*s' "${#old}" '' | tr ' ' 'a')
    printf '%s %s %s\n' "$letters" "${letters^^}" "$actor" > bytes
    compare "$format case-distinct roots" accept
    test "$(wc -l < base-roots)" -eq 2
    printf '%s %s %s\n' "${old:1}" "$zero" "$actor" > bytes
    compare "$format invalid hash width" reject
    printf '%s %s %s\n' "z${old:1}" "$zero" "$actor" > bytes
    compare "$format nonhex endpoint" reject
    printf '%040d %064d %s\n' 1 2 "$actor" > bytes
    compare "$format mixed hash widths" observe
    printf '%s %s %s\n' "$zero" "$zero" "$actor" > bytes
    compare "$format two nulls" accept
    test ! -s base-roots
    compare "$format expired deadline" reject expired
    cd ..
done
awk -v actor="$actor" 'BEGIN { for (i=1;i<=1024;i++) printf "%040d %040d %s\n", i, 0, actor }' > bytes
compare '1024 distinct roots' accept
printf '%040d %040d %s\n' 1025 0 "$actor" >> bytes
compare '1025 distinct roots' reject
truncate -s 16777217 bytes
compare 'capture cap' reject
for error in base-error gix-error hex-error; do grep -Fq "capture limit" "$error"; done
