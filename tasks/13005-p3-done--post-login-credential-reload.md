# Skip the restart after Codex login

## Problem

After a successful in-app `/codex/login` (task 27104), Phoenix writes
`~/.phoenix-ide/codex-auth.json` and the success banner says
"Restart Phoenix to start using your ChatGPT subscription."

That restart is required because `LlmConfig::from_env` constructs the
`CodexCredential` once at startup via
`codex_credential::resolve_active_auth_path()` — if no auth file existed
when the process started, `LlmConfig.codex_credential = None` and the
registry's `try_create_model` skips the ChatGPT branch entirely. The
file mtime watch inside `CodexCredential` only fires once a credential
exists; it never resurrects a `None`.

For a "tell my friends to try Phoenix with their ChatGPT account" UX,
asking the user to restart breaks the flow. They click "Sign in,"
they're done — the next message they send should just work.

## Goal

`/api/codex/login/{pkce,device}/start` → user signs in → next OpenAI
request routes through the ChatGPT bridge with no restart and no new
process.

## Approach sketches

Two reasonable shapes; pick during design:

### (a) Lazy construction at first use

`ModelRegistry` keeps an `Arc<RwLock<Option<Arc<CodexCredential>>>>` (or
similar) instead of a snapshotted `Option<Arc<CodexCredential>>`. On
each `try_create_model` call for `Provider::OpenAI`, take a read lock;
if `None`, drop the read, take the write lock, call
`resolve_active_auth_path() + CodexCredential::load`, store, drop write
lock, retry.

Pros: no plumbing required from the login handler; works for both the
in-app flow and "user ran `codex login` in another terminal"
(piggyback mode).

Cons: writes to a shared lock on every OpenAI miss until the file
appears; need to be careful about concurrent registry construction
(`ModelRegistry::new` builds the per-model `LlmService` Arcs eagerly —
that pre-binding has to either be deferred or be re-doable).

### (b) Explicit reload trigger from the login completion handler

Add `AppState::reload_codex_credential()` (or stick it on
`ModelRegistry`). Call it from `settle_pkce` / `settle_device` after a
successful write. Internally: re-run `resolve_active_auth_path()`,
`CodexCredential::load`, swap into the registry.

Pros: clean, explicit, only fires on the success path.

Cons: doesn't cover "Codex CLI wrote auth.json in another terminal"
unless we also watch the file. Have to thread the registry handle
through to the login session manager.

### (c) (a) + (b) combined

Lazy construction as the safety net, plus an explicit poke from the
login handler so the very next request picks it up without waiting for
a miss.

## Out of scope

- Logout / credential clear (separate concern)
- Mid-flight token rotation when Codex CLI's `auth.json` schema changes
  (`CodexCredential` already mtime-watches; that path is already correct)
- Hot-reloading any other bit of `LlmConfig` — keep this scoped to the
  Codex credential

## Acceptance

- [ ] In-app login on a fresh Phoenix process (no auth file at startup)
      makes the bridge active for the next OpenAI request without a
      restart.
- [ ] Concurrent OpenAI requests during the credential-load window
      either all see the new credential or all see "no creds, fall back"
      — no torn state.
- [ ] Existing `CodexCredential::get()` mtime watch behavior is
      preserved (rotating tokens after the credential is loaded still
      works).
- [ ] Ideally: `codex login` in a separate terminal (piggyback mode +
      `OPENAI_USE_CODEX_AUTH=1`) also activates without restart.

## Notes

Filed as a follow-up to 27104, where the in-app login was added but the
credential lifecycle was deliberately left as-is for that task's
scope. See the success-banner copy in
`ui/src/pages/CodexLoginPage.tsx` — that warning goes away when this
task lands.
