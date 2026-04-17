---
created: 2025-07-14
priority: p3
status: ready
artifact: improved phoenix-integration snippet + revised installation prompt
---
# Fix bugs and gaps in Phoenix OSC 133 shell integration

## Summary

The Phoenix terminal integration snippet (OSC 133 + OSC 7) and its installation
prompt have several bugs and gaps identified during a real install session.

## Context

Discovered while installing Phoenix IDE terminal HUD integration into
`~/.config/zsh/.zshrc`. Fixes 1 and 2 have already been applied to the live
snippet. The remaining items are documented here for the prompt/snippet author.

## Done When

- [ ] Snippet: double-source guard added (fixes #1 below) ✅ applied 2025-07-14
- [ ] Snippet: `exit` variable renamed to `_exit` to avoid shadowing builtin ✅ applied 2025-07-14
- [ ] Prompt: oh-my-zsh detection verifies it is *loaded*, not just *installed*
- [ ] Prompt: investigation step follows `source`/`.` chains in `.zshrc`
- [ ] Prompt: plain-git-dotfiles-repo pattern documented as a named case
- [ ] Snippet/docs: OSC 133;B absence acknowledged; tradeoffs documented
- [ ] Snippet: OSC 133;B implemented via `zle-line-init` with existing-widget chain ✅ applied 2025-07-14
  - Bug fix 2025-07-14: chained widget must be invoked with `zle __phoenix_prev_zle_line_init`, not called as a shell function — `zle -A` creates a widget alias, not a function

## Notes

### Bug 1 — double-source appends duplicates to hook arrays (FIXED)

Original:
```zsh
typeset -ag precmd_functions preexec_functions
precmd_functions+=(__phoenix_precmd)
preexec_functions+=(__phoenix_preexec)
```
`+=` is not guarded. If `.zshrc` is sourced twice in a running shell, both
functions get appended again and fire twice per command.

Fix applied:
```zsh
typeset -ag precmd_functions preexec_functions
(( ${precmd_functions[(Ie)__phoenix_precmd]}   )) || precmd_functions+=(__phoenix_precmd)
(( ${preexec_functions[(Ie)__phoenix_preexec]} )) || preexec_functions+=(__phoenix_preexec)
```
`(Ie)` is zsh's exact-match subscript flag; the arithmetic returns 0 (falsy) if
the function is not already present.

### Bug 2 — `exit` variable shadows the zsh builtin (FIXED)

`local exit=$?` works today but shadows `exit` within the function scope.
Rename to `_exit` (or `exit_code`) — zero-cost defensive change.

### Prompt gap — oh-my-zsh detection is install-presence, not load-presence

The prompt routes to `~/.oh-my-zsh/custom/` when `~/.oh-my-zsh/` exists.
On this machine oh-my-zsh is installed system-wide but never sourced — the
drop-in was written to a dead directory. The check should be:
```bash
[ -d ~/.oh-my-zsh ] && grep -q 'oh-my-zsh\.sh' ~/.zshrc
```

### Prompt gap — investigation does not follow `source` chains

The prompt inspects `~/.zshrc` for framework markers, but this machine's
`~/.zshrc` only contains `source ~/.config/zsh/.zshrc`. All real config
(p10k, history, aliases) lives in the sourced file. The investigation step
should follow one level of `source`/`.` directives to find the real config.

### Prompt gap — plain-git dotfiles repo not a named case

The prompt names chezmoi, yadm, stow, home-manager — but not the common
pattern of `~/.config` (or `~/dotfiles`) being a plain `git init` repo that
tracks files by path. This case gets silently caught by the symlink fallback
but deserves its own named branch: edit in-place, stage, ask before commit.

### Feature gap — OSC 133;B not emitted

The full sequence is A (prompt start) → B (prompt end / input start) → C
(command executing) → D (command done). Without B, the terminal cannot
delineate prompt text from typed command — relevant for click-to-rerun and
input-region selection features.

B must fire *after* the prompt is rendered but *before* readline takes over,
which requires a `zle` hook or embedding in `$PROMPT` — not a `precmd` hook.
For p10k users this is complex because p10k owns `$PROMPT` entirely. The
simplest workaround (`PS1+=$'\e]133;B\e\\'`) is fragile with p10k's prompt
management. Recommend the prompt documentation acknowledges this gap and
suggests the zle approach for non-p10k users.
