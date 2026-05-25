/**
 * Shell integration snippets. Each paste enables OSC 133 (command lifecycle)
 * and OSC 7 (cwd reporting) for the user's shell. Users paste into their rc
 * file and re-source it (or restart their shell).
 *
 * REQ-TERM-017: snippets are tailored per shell so the user gets a single
 * one-step paste rather than a multi-shell soup.
 *
 * Escape-sequence note: in shell printf strings we want the literal bytes
 * `ESC ] 133 ; A ESC \` (the ST terminator). The TS string literals below
 * use `\\e` so the rendered text contains a literal backslash-e (which
 * printf interprets as ESC), and `\\\\` to render a literal backslash-backslash
 * (which printf interprets as a single backslash, completing the ST). When
 * pasted into a real shell, this produces the correct OSC 133 byte sequence.
 */

export interface ShellSnippet {
  shellName: string;
  rcFile: string;
  snippet: string;
}

export const ZSH_SNIPPET: ShellSnippet = {
  shellName: 'zsh',
  rcFile: '~/.zshrc',
  snippet: `# Phoenix terminal integration (OSC 133 A/B/C/D + OSC 7)
__phoenix_prompt_start() { printf '\\e]133;A\\e\\\\' }
__phoenix_preexec() { printf '\\e]133;C;%s\\e\\\\' "$1" }
__phoenix_precmd() {
  # _exit, not 'exit': 'exit' would shadow the zsh builtin inside this function.
  local _exit=$?
  printf '\\e]133;D;%d\\e\\\\' $_exit
  printf '\\e]7;file://%s%s\\e\\\\' "\${HOST}" "$PWD"
  __phoenix_prompt_start
}
typeset -ag precmd_functions preexec_functions
# Idempotent re-add: \`+=\` without a presence guard appends a duplicate
# every time .zshrc is sourced, so each command would fire the hooks twice.
# (Ie) is the exact-match subscript flag; the arithmetic is 0 (falsy) when
# the function is not already present.
(( \${precmd_functions[(Ie)__phoenix_precmd]} ))   || precmd_functions+=(__phoenix_precmd)
(( \${preexec_functions[(Ie)__phoenix_preexec]} )) || preexec_functions+=(__phoenix_preexec)
# OSC 133;B (prompt end / input start) requires a zle hook — precmd fires
# before the prompt is rendered, so it cannot mark the input region.
# Chain to any pre-existing zle-line-init widget via \`zle -A\` (which
# creates a widget *alias*, not a function — invoke it with \`zle <name>\`,
# not as a shell function call). The outer guard prevents re-sourcing from
# aliasing our own widget to itself and infinite-looping on the next key.
if [[ -z "$__phoenix_zle_line_init_installed" ]]; then
  typeset -g __phoenix_zle_line_init_installed=1
  zle -A zle-line-init __phoenix_prev_zle_line_init 2>/dev/null
  __phoenix_zle_line_init() {
    zle __phoenix_prev_zle_line_init 2>/dev/null
    printf '\\e]133;B\\e\\\\'
  }
  zle -N zle-line-init __phoenix_zle_line_init
fi
__phoenix_prompt_start`,
};

export const BASH_SNIPPET: ShellSnippet = {
  shellName: 'bash',
  rcFile: '~/.bashrc',
  // OSC 133;B (prompt end / input start) is intentionally omitted on bash:
  // emitting it correctly requires either a readline binding via \`bind -x\`
  // or embedding the sequence in \`$PS1\` itself, and the \`$PS1\` route is
  // fragile under prompt frameworks (powerline-bash, starship in some
  // modes, etc.) that own the prompt string. The A/C/D markers + OSC 7
  // cover the command-lifecycle and cwd-reporting features Phoenix relies
  // on today.
  snippet: `# Phoenix terminal integration (OSC 133 A/C/D + OSC 7; no 133;B — see note in source)
__phoenix_prompt_start() { printf '\\e]133;A\\e\\\\'; }
__phoenix_preexec() {
  [[ -n "$COMP_LINE" ]] && return
  [[ "$BASH_COMMAND" == "$PROMPT_COMMAND" ]] && return
  printf '\\e]133;C;%s\\e\\\\' "$BASH_COMMAND"
}
__phoenix_precmd() {
  # _exit, not 'exit': 'exit' would shadow the bash builtin inside this function.
  local _exit=$?
  printf '\\e]133;D;%d\\e\\\\' $_exit
  printf '\\e]7;file://%s%s\\e\\\\' "\${HOSTNAME}" "$PWD"
  __phoenix_prompt_start
}
# Idempotent install: only prepend our precmd if it isn't already
# present somewhere in PROMPT_COMMAND, otherwise re-sourcing .bashrc
# fires the hook multiple times per prompt.
case ";$PROMPT_COMMAND;" in
  *";__phoenix_precmd;"*) ;;
  *) PROMPT_COMMAND='__phoenix_precmd'\${PROMPT_COMMAND:+;$PROMPT_COMMAND} ;;
esac
trap '__phoenix_preexec' DEBUG
__phoenix_prompt_start`,
};

export const FISH_SNIPPET: ShellSnippet = {
  shellName: 'fish',
  rcFile: '~/.config/fish/config.fish',
  snippet: `# Phoenix terminal integration (OSC 133 + OSC 7)
function __phoenix_prompt_start --on-event fish_prompt
    printf '\\e]133;A\\e\\\\'
    printf '\\e]7;file://%s%s\\e\\\\' (hostname) "$PWD"
end
function __phoenix_preexec --on-event fish_preexec
    printf '\\e]133;C;%s\\e\\\\' "$argv"
end
function __phoenix_postexec --on-event fish_postexec
    printf '\\e]133;D;%d\\e\\\\' $status
end`,
};

/** Resolve a shell path (e.g. "/bin/zsh") to its snippet, or null if unsupported. */
export function getSnippetForShell(shellPath: string | null | undefined): ShellSnippet | null {
  if (!shellPath) return null;
  const base = shellPath.split('/').pop()?.toLowerCase() ?? '';
  switch (base) {
    case 'zsh':
      return ZSH_SNIPPET;
    case 'bash':
      return BASH_SNIPPET;
    case 'fish':
      return FISH_SNIPPET;
    default:
      return null;
  }
}

/** Display name for a shell path, falling back to "your shell". */
export function shellDisplayName(shellPath: string | null | undefined): string {
  if (!shellPath) return 'your shell';
  const base = shellPath.split('/').pop()?.toLowerCase() ?? '';
  if (!base) return 'your shell';
  return base;
}
