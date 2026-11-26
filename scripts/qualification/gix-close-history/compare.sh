#!/usr/bin/env bash
set -euo pipefail
probe=$1
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
export GIT_AUTHOR_NAME=Fixture GIT_AUTHOR_EMAIL=fixture@example.test
export GIT_COMMITTER_NAME=Fixture GIT_COMMITTER_EMAIL=fixture@example.test

for format in sha1 sha256; do
    git init -q -b main --object-format="$format" "$scratch/$format"
    cd "$scratch/$format"
    log="$scratch/$format.log"
    : > "$log"
    test "$("$probe" . < "$log")" = 'unborn refs/heads/main'
    git commit -qm base --allow-empty
    base=$(git rev-parse HEAD)
    test "$("$probe" . < "$log")" = "attached refs/heads/main $base"
    git worktree add -q --detach "$scratch/worktree-$format"
    cd "$scratch/worktree-$format"
    git commit -qm ancestor --allow-empty
    ancestor=$(git rev-parse HEAD)
    git commit -qm old-only --allow-empty
    old=$(git rev-parse HEAD)
    git switch -q --detach "$base"
    git commit -qm new-only --allow-empty
    new=$(git rev-parse HEAD)
    git switch -q --detach "$base"
    git update-ref refs/temporary/old "$old"
    git update-ref refs/temporary/new "$new"
    zero=${base//?/0}
    printf '%s %s Fixture <fixture@example.test> 1600000000 +0000\tfixture\n' "$old" "$zero" "$zero" "$new" > "$log"
    compare() {
        local roots=("$base" "$old" "$new") durable=(--branches --remotes --tags) status=0
        git show-ref --verify --quiet refs/stash || status=$?
        case $status in
            0) durable+=(refs/stash) ;;
            1) ;;
            *) return "$status" ;;
        esac
        {
            echo "detached $base"
            git rev-list "${roots[@]}" --not "${durable[@]}" | LC_ALL=C sort
        } > "$scratch/expected"
        "$probe" . < "$log" > "$scratch/actual"
        diff -u "$scratch/expected" "$scratch/actual"
    }
    compare
    for oid in "$ancestor" "$old" "$new"; do grep -Fxq "$oid" "$scratch/actual"; done
    git tag -a -m preserved old "$old"
    git update-ref refs/remotes/example/new "$new"
    git pack-refs --all
    compare
    test "$(wc -l < "$scratch/actual")" -eq 1
    git tag -d old >/dev/null
    git update-ref refs/stash "$old"
    compare
    test "$(wc -l < "$scratch/actual")" -eq 1
    if "$probe" . 0 < "$log"; then echo 'budget failure accepted' >&2; exit 1; fi
    if printf 'incomplete' | "$probe" .; then echo 'partial capture accepted' >&2; exit 1; fi
    echo "$format: loose, packed, old-only, new-only, ancestor, stash, unborn and error cases checked"
done
cd "$scratch"
if git init -q --ref-format=reftable "$scratch/reftable"; then
    if "$probe" "$scratch/reftable" < /dev/null; then echo 'reftable accepted' >&2; exit 1; fi
else
    echo 'SKIP reftable: unavailable in runner Git'
fi
