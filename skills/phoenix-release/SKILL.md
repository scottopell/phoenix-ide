---
name: phoenix-release
description: Cut a new phoenix-ide version, push the tag, let GitHub Actions build and publish the binary, and replace the auto-generated release notes with a sub-agent-drafted, human-reviewable changelog. Use when the user says "cut a release", "publish a version", "ship vX.Y.Z", "tag a new release", or asks how to publish.
---

# Phoenix IDE Release

End-to-end: version bump → tag → CI build → polished release notes posted via `gh release edit`.

The CI half is fully automated (`.github/workflows/release.yml` fires on `v[0-9]+.*` tag push). The interesting work is (1) the version bump and (2) writing notes that a human would actually read.

## Preconditions

- Clean working tree on `main`, in sync with origin.
- Local `gh` authenticated with push + release-edit rights on `scottopell/phoenix-ide`.
- The user has explicitly authorized the release. Push and tag operations are shared-state — never proceed without confirmation.

## Step 1 — Decide the version

Read current version from `crates/phoenix-ide/Cargo.toml` (the root `Cargo.toml` is workspace-only since the `crates/` restructure — it has no `[package]` block).

```bash
grep -m1 '^version' crates/phoenix-ide/Cargo.toml
git tag --sort=-creatordate | head -5
git rev-list <last-tag>..HEAD --count   # how much accumulated
```

Default bump is minor (`0.X.0 → 0.X+1.0`). Confirm with the user — never auto-pick the major bump. The pre-1.0 convention here is: minor for features and breaking-but-low-impact changes; only call something a major bump if there's a deliberate compatibility break the user has named.

## Step 2 — Bump version, commit, tag, push (requires user authorization)

Push and tag operations affect shared state and trigger CI. Get explicit go-ahead before running.

The shortest path is the helper script, which does all four steps (bump `crates/phoenix-ide/Cargo.toml`, commit `chore: bump version to X.Y.Z`, annotate tag `vX.Y.Z`, push `main` + tag):

```bash
./scripts/tag-release.sh vX.Y.Z
```

Or, if you prefer to do it by hand for visibility:

```bash
# Edit crates/phoenix-ide/Cargo.toml: version = "X.Y.Z"
cargo update -p phoenix_ide --offline
git add crates/phoenix-ide/Cargo.toml Cargo.lock
git commit -m "chore: bump version to X.Y.Z"
git push origin main
git tag vX.Y.Z
git push origin vX.Y.Z
```

Either way, the tag push fires `.github/workflows/release.yml`.

Tag push fires `.github/workflows/release.yml` → ~7 minutes on the typical history.

## Step 3 — Wait for the build, verify the release

```bash
gh run watch $(gh run list --workflow=release.yml --limit 1 --json databaseId -q '.[0].databaseId') --exit-status
gh release view vX.Y.Z --json url,assets -q '{url, assets: [.assets[].name]}'
```

Expect: status `success`, one asset `phoenix_ide-x86_64-unknown-linux-musl`. The release body at this point is GitHub's auto-generated "What's Changed" list — keep it as a fallback but replace it in the next step.

If the build fails, do not retry blindly. Open the run, read the failed step, fix the underlying issue, push, and let the next tag re-trigger. Never `--force` a tag — cut a patch version instead.

## Step 4 — Draft polished release notes via sub-agent

The auto-generated notes are a commit dump. For any release with >20 commits, swap in a sub-agent-drafted writeup.

**Spawn a `general-purpose` agent with the verbatim prompt in [`release-notes-prompt.md`](release-notes-prompt.md).** Substitute the version pair (`v0.6.0..v0.7.0` etc.) and the release URL. The prompt does its own investigation: walks the commit range, reads PR bodies, checks `tasks/` for richer context, skims new `specs/` dirs, and emits markdown ready to pipe into `gh release edit`.

The prompt deliberately:

- Tells the agent *not* to post the notes — output text only, you review before posting.
- Constrains length and structure (Highlights / New features / Fixes / Performance / Under the hood / Upgrading / Full changelog).
- Demands a separate "judgment calls" section so you know what was bucketed where.

## Step 5 — Verify the agent's flagged uncertainties

The sub-agent will end with a "judgment calls" paragraph naming anything it inferred or couldn't confirm. **Verify each one before posting.** Typical things to check:

- **Migration/schema claims** — grep `crates/phoenix-ide/src/db/migrations.rs` and `crates/phoenix-ide/src/db.rs` for any column or table the notes mention. (Some columns are added via the older init-time `ALTER TABLE … IF NOT EXISTS` pattern in `db.rs`, not the versioned runner in `db/migrations.rs`; both run on startup, but the wording of "table vs column" matters.)
- **Env var / config claims** — grep the named identifier in `crates/`. Confirm the semantic the notes describe matches the code.
- **"Removed" claims** — `git log --diff-filter=D --summary v<prev>..HEAD` to confirm files actually gone.

Fix any wording the verification surfaces. Don't post inaccurate notes — the changelog is the highest-visibility doc in the repo.

## Step 6 — Prepend the AI-generated banner and post

Every AI-drafted release body **must** begin with this banner verbatim, before any other content:

```markdown
> _The notes below this line are AI-generated from the v<prev>..v<new> commit and PR history. A human exec-summary may appear above this banner._
```

The banner exists so the user can optionally write their own exec summary above it later without retroactively making the AI section look human-authored. Don't move it, don't reword it, don't drop it on the grounds that "the user will know" — the audience for the banner is future readers, not the current session.

Then post:

```bash
gh release edit vX.Y.Z --notes-file /tmp/v<X.Y.Z>-notes.md
gh release view vX.Y.Z --json url -q .url
```

Print the URL back to the user. Done.

## Anti-patterns

- **Bumping the version in a separate PR before the tag.** The bump commit *is* the tagged commit — that's why every prior release matches the pattern `chore: bump version to X.Y.Z` → tag at HEAD.
- **Force-pushing a tag to "fix" notes.** Re-edit the release body via `gh release edit` — the tag and binary stay valid.
- **Letting the sub-agent post directly.** It does not see the verification step. Always human-review then post.
- **Dropping the AI banner because "this one's really good".** It's a structural marker, not a quality disclaimer.

## Related skills

- `phoenix-deployment` — deploying a built release to production (separate from publishing it on GitHub).
- `phoenix-development` — `./dev.py up/check` etc., used during the optional pre-release sanity build.

## Open follow-ups

- No checksum file or macOS/aarch64 binary is produced. If multi-arch builds matter, that's a workflow expansion, not a process gap.
