Reduce GitHub Actions CI wall time (crept to ~10min on PRs) and stop main breaking silently.

Root-cause findings (measured):
- PR CI ~9-10min, main ~1min. Asymmetry = rust-cache bucket warmth.
- cargo `--timings`: first-party phoenix_ide is only ~15s. Compile floor is DEPS: chromiumoxide+cdp ~63s, aws-lc-sys ~24s, tokio ~12s.
- Narrowing codegen by package is DEAD: phoenix_ide is dep-graph root, drags whole workspace (measured ~0 saving).
- Cache bug: key `profile-${hashFiles(Cargo.toml)}` + no restore-keys => any Cargo.toml delta exiles a branch to its own cold bucket that never warms (save skipped when key exists). main fast only because it reads/writes one warm bucket.
- aws-lc-sys (24s) is an unintended feature-unification leak: phoenix-ide chose ring but chromiumoxide`s reqwest 0.13 pulls aws-lc; both crypto backends compile. Also two reqwest versions (0.12 + 0.13).

Work (this PR): 
1. Crypto: unify on aws-lc-rs, drop ring (bump phoenix-ide reqwest 0.12->0.13 to unify reqwest version too).
2. Cache fix: rust-cache shared-key + restore-keys fallback + save-if main (PRs warm-start from main deps).
3. main-failure auto-issue (dedup by title, close-on-green).
4. dev.py lane reorder: isolate clippy in own CARGO_TARGET_DIR so test compile reuses codegen.
5. sccache (GHA backend) as compilation-cache backstop.

Deferred (interactive): e2e trim (now the ~304s floor).

---

## Implementation notes (as delivered)

1. Crypto (commit "build: unify TLS on aws-lc-rs"): switched rustls/tokio-rustls
   features ring -> aws_lc_rs, main.rs provider install ring -> aws_lc_rs, and
   bumped reqwest 0.12 -> 0.13 (feature renamed `rustls-tls` -> `rustls`). This
   unifies to a single reqwest version (kills the duplicate compile) and makes
   aws-lc-rs the runtime provider. Build clean, no API breaks.
   - SCOPE CALL: ring is NOT fully gone — it remains transitively via `rcgen`
     (cert gen in phoenix-tls) and the `rustls-webpki`/`rustls-platform-verifier`
     verification chain. Excising it needs fragile transitive feature overrides
     on 3 crates for ~3.6s of compile the cache absorbs. Intent (aws-lc-rs
     primary, no openssl, no duplicate reqwest) is met; left transitive ring.

2. Cache fix (ci.yml): replaced `key: profile-${hashFiles('Cargo.toml')}` with
   `shared-key: check` + `prefix-key: v1-rust` + `save-if: main`. One warm bucket
   maintained by main; PRs restore it read-only via rust-cache's restore-key
   prefix, warm-starting from main's deps. Profile edits need a manual
   prefix-key bump (the only change that silently invalidates a restored
   target/) — documented inline.

3. Lane reorder (dev.py): chose REORDER over separate clippy target dir.
   Root cause of the redundant `cargo test compile` was `cargo clippy`
   (RUSTC_WORKSPACE_WRAPPER=clippy-driver) running BETWEEN codegen and the test
   build, rewriting workspace fingerprints. Moving clippy to last lets the test
   build reuse codegen (cache hit) with no separate target dir and no
   sccache-warmth dependency / cold-dir regression risk. Note: wall-clock win is
   gated on the e2e trim — e2e (~304s) is the lane floor, so this is a
   CPU/compute saving (~140s) until e2e shrinks.

4. main-failure (ci.yml): `notify-main-failure` opens/comments a deduped
   "CI failing on main" issue on push-to-main failure; `close-main-failure`
   closes it on the next green. Dedup by exact title match.

5. sccache (ci.yml): mozilla-actions/sccache-action + RUSTC_WRAPPER=sccache +
   SCCACHE_GHA_ENABLED at job env. Content-addressed compilation cache; backs up
   the rust-cache target/ snapshot for cases it misses (Cargo.lock changes).
   Coexists with rust-cache (target snapshot + registry/bin); slight storage
   redundancy under the 10GB GHA limit, tune later if eviction shows up.

CI-wire changes (cache/sccache/notify) can't be validated locally — verified via
the PR's own CI run + codex review.
