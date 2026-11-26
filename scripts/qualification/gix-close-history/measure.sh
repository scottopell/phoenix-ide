#!/usr/bin/env bash
set -euo pipefail
probe=$1
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
export GIT_AUTHOR_NAME=Fixture GIT_AUTHOR_EMAIL=fixture@example.test
export GIT_COMMITTER_NAME=Fixture GIT_COMMITTER_EMAIL=fixture@example.test
for size in 1 25000; do
    git init -q -b main "$scratch/$size"
    cd "$scratch/$size"
    awk -v count="$size" 'BEGIN {
        for (i=1; i<=count; i++) {
            print "commit refs/heads/main\ncommitter Fixture <fixture@example.test> " (1600000000+i) " +0000\ndata 1\nx\n"
        }
        print "done"
    }' | git fast-import --quiet
    git switch -q --detach main
    git commit -qm unpreserved --allow-empty
    tip=$(git rev-parse HEAD)
    git commit-graph write --reachable
    for sample in 1 2 3 4 5; do
        echo "commits=$size sample=$sample"
        /usr/bin/time -f 'git elapsed_s=%e peak_rss_kib=%M' git rev-list "$tip" --not --branches --remotes --tags > "$scratch/git"
        /usr/bin/time -f 'gix elapsed_s=%e peak_rss_kib=%M' "$probe" . < /dev/null > "$scratch/gix"
        test "$(head -1 "$scratch/gix")" = "detached $tip"
        tail -n +2 "$scratch/gix" | diff -u "$scratch/git" -
    done
    if [[ $size -gt 1000 ]]; then
        if "$probe" . 1000 < /dev/null > "$scratch/gix" 2> "$scratch/error"; then
            echo 'accepted beyond graph budget' >&2; exit 1
        fi
        grep -Fq 'graph/ref budget exhausted' "$scratch/error"
        test ! -s "$scratch/gix"
    fi
done
