# Command Suggestion — Design

## Request flow

```
phx "<query>"            (in a Phoenix terminal; phx is on PATH)
  └─ reads PHOENIX_API_URL + PHOENIX_SUGGEST_TOKEN from its env
  └─ POST {api_url}/api/suggest
         header X-Phoenix-Suggest-Token: <token>
         body   { "query": "<query>", "model"?: "<id>" }
  ◄─ { "commands": ["…", "…"] }
  └─ prints each as an OSC 8 phxrun: link
         terminal renders it → click drops the command on the prompt
```

## Wire shapes

Defined in `api::types`:

- `SuggestRequest { query: String, model: Option<String> }`
- `SuggestResponse { commands: Vec<String> }`

`model` is omitted by default; when present it overrides the cheap-model
default. `commands` is the ordered list of runnable command lines.

## The endpoint (`suggest_handler`)

Authorization runs first: the handler compares `X-Phoenix-Suggest-Token`
against `AppState.suggest_token` and returns `403` on mismatch or absence. The
endpoint is on the password middleware's exempt list (`api::auth::is_exempt_path`),
so the token is the *only* gate — see `specs/auth` for why an exempt endpoint
must carry its own credential.

It then resolves the model (`get_cheap_model` or `get(model)`), and delegates
to `crate::suggest::suggest_commands`, which builds a minimal `LlmRequest`
(the suggester system prompt, the query, no tools, a small token budget, a
shared cache key), calls `complete()` under a timeout, and parses the response
into command lines — dropping blank, comment (`#`), and stray code-fence lines.
The shape mirrors `crate::title_generator`: a lightweight auxiliary one-shot
caller over `LlmService::complete` (`specs/llm`).

## Token lifecycle (`resolve_suggest_token`)

At startup the server resolves the token before constructing `AppState`:

1. Read `app_settings` keys `suggest_token` and
   `suggest_token_password_fingerprint`.
2. Reuse the stored token when it is non-empty and its fingerprint equals the
   current password fingerprint (`api::auth::password_fingerprint`, empty when
   no password is set).
3. Otherwise mint a fresh 256-bit token, and store it alongside the current
   fingerprint.

Persistence failures degrade to a per-process token (logged), never fatal. The
fingerprint binding mirrors `SessionToken` in `specs/auth`: a normal restart
keeps the same token (so terminals opened before it stay authorized), while a
password change re-mints.

## `phx` delivery and dispatch

`phx` is a symlink at `<data_dir>/bin/phx` pointing to the running server
binary (`install_phx_symlink`), refreshed each startup so it tracks upgrades.
`main` dispatches on invocation name: when argv[0]'s basename is `phx` (or the
first argument is `suggest`), it runs `phx_cli::run` and exits instead of
starting the server. The client reads the query from arguments or stdin, posts
to `/api/suggest` over loopback (accepting the server's self-signed TLS), and
emits the run-links.

## Environment injection (`PtyEnvInjection`)

A process-global injection, installed once at startup (`setup_phx_companion` →
`set_pty_env_injection`), carries the `phx` bin directory and the
`PHOENIX_API_URL` / `PHOENIX_SUGGEST_TOKEN` pairs. It is applied at two spawn
points, because a tmux pane shell inherits the tmux *server's* environment, not
the `tmux attach` client's:

- Direct shell PTY: `build_env` prepends the bin dir to `PATH` and appends the
  vars (`specs/terminal` REQ-TERM-002).
- Tmux server: `apply_pty_env_injection` adds the same to the `new-session`
  command (`specs/tmux-integration`).

The injection is enumerated and deliberate — not blind inheritance of the
server environment.

## OSC 8 run-links

`phx` emits each command as `ESC ] 8 ; ; phxrun:<base64(command)> ST <command>
ESC ] 8 ; ; ST`. The base64 URI keeps arbitrary command bytes out of the
escape sequence's control range. The terminal UI's `linkHandler` intercepts the
`phxrun:` scheme, decodes the command, and writes it to the PTY input without a
trailing newline (`specs/terminal-panel`). For the tmux path, the server config
advertises the `hyperlinks` terminal feature so tmux forwards the OSC 8 wrapper
to the client rather than stripping it (`specs/tmux-integration`).

## Design Decisions

**Why a capability token rather than loopback trust.** Trusting any same-host
caller would, on a password-protected or shared host, let any local process
burn LLM tokens. A scoped token — injected only into terminal environments — is
possessed by `phx` and not by arbitrary local processes, and it carries no
authority beyond suggestion.

**Why stateless one-shot rather than a conversation.** A conversation per
suggestion is overkill and pollutes history. The base tier needs no context;
richer tiers (retrieval scoped to a terminal's conversation lineage, or a full
agent turn) are additive on the same endpoint, which is why it accepts an
optional model and is gated by a token rather than conversation ownership.

**Why place the command on the prompt, not run it.** Auto-executing
model-authored text in the user's shell removes the review beat. Dropping it on
the prompt keeps the human in the loop and runs it in their own interactive
shell with their own environment.
