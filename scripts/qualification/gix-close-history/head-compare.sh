#!/usr/bin/env bash
set -euo pipefail
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
base=$PWD/decoder-build/head-cli
candidate=$PWD/target/release/typed-head
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C
export GIT_AUTHOR_NAME=Fixture GIT_AUTHOR_EMAIL=fixture@example.test
export GIT_COMMITTER_NAME=Fixture GIT_COMMITTER_EMAIL=fixture@example.test
cd "$scratch"
git init -qb main repo
admin=$scratch/repo/.git
head=$admin/HEAD
write_head() { printf '%s\n' "$1" > "$head"; }
compare() {
    local name=$1 a=0 b=0
    shift
    if [[ -f $head ]]; then cp "$head" before; else rm -f before; fi
    "$base" "$scratch/repo" "$@" > cli-out 2> cli-err || a=$?
    if [[ -f before ]]; then cp before "$head"; else rm -f "$head"; fi
    "$candidate" "$scratch/repo" "$@" > gix-out 2> gix-err || b=$?
    [[ $a -le 1 && $b -le 1 ]]
    printf '%s: CLI[%s]=%s gix[%s]=%s\n' "$name" "$a" "$(cat cli-out cli-err)" "$b" "$(cat gix-out gix-err)"
    case $name in
        unborn|attached|detached|recursive-chain|dangling-chain|missing-branch-with-dwim-decoy|ambiguous-HEAD-with-tag) test "$a" -eq 0 && test "$b" -eq 0 && cmp cli-out gix-out ;;
        missing-nonbranch) test "$a" -eq 1 && test "$b" -eq 0 && grep -Fxq "unborn refs/tags/absent" gix-out ;;
        cyclic-chain|broken-ref|malformed-HEAD|missing-HEAD) test "$a" -eq 1 && test "$b" -eq 1 ;;
        changing-unborn) test "$a" -eq 1 && test "$b" -eq 1 && grep -Fq 'changed' cli-err && grep -Fq 'changed' gix-err ;;
    esac || return 1
}
compare unborn
compare changing-unborn refs/heads/other
write_head 'ref: refs/heads/main'
git -C repo commit -qm base --allow-empty
compare attached
commit=$(git -C repo rev-parse HEAD)
write_head "$commit"
compare detached
write_head 'ref: refs/heads/one'
printf 'ref: refs/heads/two\n' > "$admin/refs/heads/one"
printf 'ref: refs/heads/main\n' > "$admin/refs/heads/two"
compare recursive-chain
printf 'ref: refs/heads/absent\n' > "$admin/refs/heads/two"
compare dangling-chain
printf 'ref: refs/heads/one\n' > "$admin/refs/heads/two"
compare cyclic-chain
write_head 'ref: refs/heads/broken'
printf 'not an object id\n' > "$admin/refs/heads/broken"
compare broken-ref
write_head 'ref: refs/tags/absent'
compare missing-nonbranch
write_head 'ref: refs/heads/absent'
mkdir -p "$admin/refs/refs/heads"
printf '%s\n' "$commit" > "$admin/refs/refs/heads/absent"
compare missing-branch-with-dwim-decoy
write_head 'ref: refs/heads/main'
printf '%s\n' "$commit" > "$admin/refs/tags/HEAD"
compare ambiguous-HEAD-with-tag
write_head 'not a ref or object id'
compare malformed-HEAD
rm "$head"
compare missing-HEAD
