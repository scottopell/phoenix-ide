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
  snippet: `# Phoenix terminal integration (OSC 133 + OSC 7)
__phoenix_prompt_start() { printf '\\e]133;A\\e\\\\' }
__phoenix_preexec() { printf '\\e]133;C;%s\\e\\\\' "$1" }
__phoenix_precmd() {
  local exit=$?
  printf '\\e]133;D;%d\\e\\\\' $exit
  printf '\\e]7;file://%s%s\\e\\\\' "\${HOST}" "$PWD"
  __phoenix_prompt_start
}
typeset -ag precmd_functions preexec_functions
precmd_functions+=(__phoenix_precmd)
preexec_functions+=(__phoenix_preexec)
__phoenix_prompt_start`,
};

export const BASH_SNIPPET: ShellSnippet = {
  shellName: 'bash',
  rcFile: '~/.bashrc',
  snippet: `# Phoenix terminal integration (OSC 133 + OSC 7)
__phoenix_prompt_start() { printf '\\e]133;A\\e\\\\'; }
__phoenix_preexec() {
  [[ -n "$COMP_LINE" ]] && return
  [[ "$BASH_COMMAND" == "$PROMPT_COMMAND" ]] && return
  printf '\\e]133;C;%s\\e\\\\' "$BASH_COMMAND"
}
__phoenix_precmd() {
  local exit=$?
  printf '\\e]133;D;%d\\e\\\\' $exit
  printf '\\e]7;file://%s%s\\e\\\\' "\${HOSTNAME}" "$PWD"
  __phoenix_prompt_start
}
PROMPT_COMMAND='__phoenix_precmd'\${PROMPT_COMMAND:+;$PROMPT_COMMAND}
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
