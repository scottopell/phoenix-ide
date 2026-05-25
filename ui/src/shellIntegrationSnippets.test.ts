import { describe, expect, test } from 'vitest';
import {
  BASH_SNIPPET,
  FISH_SNIPPET,
  ZSH_SNIPPET,
  getSnippetForShell,
  shellDisplayName,
} from './shellIntegrationSnippets';

/**
 * Regression guards for the OSC 133 / OSC 7 shell-integration snippets
 * (task 02652).
 *
 * Each test pins a known-bad pattern (the bug the task documented) so a
 * future edit that re-introduces it surfaces here before reaching a
 * user's `.zshrc` / `.bashrc`.
 */

describe('ZSH_SNIPPET', () => {
  test('does not shadow the zsh `exit` builtin', () => {
    // `local exit=$?` is the bug from task 02652. Use `_exit` (or any other
    // non-builtin name) so the builtin stays callable inside the function.
    expect(ZSH_SNIPPET.snippet).not.toMatch(/\blocal\s+exit=/);
    expect(ZSH_SNIPPET.snippet).toMatch(/\blocal\s+_exit=\$\?/);
  });

  test('idempotent precmd_functions / preexec_functions install', () => {
    // Bare `precmd_functions+=(__phoenix_precmd)` re-appends on every
    // .zshrc source, firing the hook twice per command. The (Ie) presence
    // check is the fix the task validated.
    expect(ZSH_SNIPPET.snippet).not.toMatch(
      /^precmd_functions\+=\(__phoenix_precmd\)$/m,
    );
    expect(ZSH_SNIPPET.snippet).toContain(
      '${precmd_functions[(Ie)__phoenix_precmd]}',
    );
    expect(ZSH_SNIPPET.snippet).toContain(
      '${preexec_functions[(Ie)__phoenix_preexec]}',
    );
  });

  test('installs OSC 133;B via zle-line-init with a presence guard', () => {
    // The chained widget must be invoked with `zle <name>`, not as a shell
    // function call — `zle -A` creates a widget alias, not a function.
    expect(ZSH_SNIPPET.snippet).toContain('zle -A zle-line-init __phoenix_prev_zle_line_init');
    expect(ZSH_SNIPPET.snippet).toContain('zle __phoenix_prev_zle_line_init');
    // OSC 133;B byte (`\e]133;B\e\\`) must appear inside the zle widget.
    expect(ZSH_SNIPPET.snippet).toMatch(/\\e\]133;B\\e\\\\/);
    // The outer install guard prevents re-sourcing from aliasing our own
    // widget to itself (which would infinite-loop on the next key).
    expect(ZSH_SNIPPET.snippet).toContain('$__phoenix_zle_line_init_installed');
  });

  test('emits OSC 133 A/C/D + OSC 7 markers', () => {
    expect(ZSH_SNIPPET.snippet).toMatch(/\\e\]133;A\\e\\\\/);
    expect(ZSH_SNIPPET.snippet).toMatch(/\\e\]133;C;%s\\e\\\\/);
    expect(ZSH_SNIPPET.snippet).toMatch(/\\e\]133;D;%d\\e\\\\/);
    expect(ZSH_SNIPPET.snippet).toMatch(/\\e\]7;file:\/\//);
  });
});

describe('BASH_SNIPPET', () => {
  test('does not shadow the bash `exit` builtin', () => {
    expect(BASH_SNIPPET.snippet).not.toMatch(/\blocal\s+exit=/);
    expect(BASH_SNIPPET.snippet).toMatch(/\blocal\s+_exit=\$\?/);
  });

  test('idempotent PROMPT_COMMAND install', () => {
    // The PROMPT_COMMAND prepend must be guarded — re-sourcing without a
    // guard would accumulate `__phoenix_precmd;__phoenix_precmd;…`.
    // Pin the guard structure (case-pattern + skip-branch + prepend
    // fallback) so a regression that removes the guard but still
    // happens to contain `;__phoenix_precmd;` somewhere fails the test.
    expect(BASH_SNIPPET.snippet).toContain('case ";$PROMPT_COMMAND;" in');
    expect(BASH_SNIPPET.snippet).toMatch(/\*";__phoenix_precmd;"\*\)\s*;;/);
    expect(BASH_SNIPPET.snippet).toMatch(
      /PROMPT_COMMAND='__phoenix_precmd'\$\{PROMPT_COMMAND:\+;\$PROMPT_COMMAND\}/,
    );
  });

  test('does not claim to emit OSC 133;B', () => {
    // OSC 133;B on bash needs a `bind -x` readline binding or a `$PS1`
    // embedding that's fragile under prompt frameworks. Confirm we are
    // intentionally not shipping it here so the regression test catches
    // a partial implementation that emits B without the surrounding
    // plumbing.
    expect(BASH_SNIPPET.snippet).not.toMatch(/\\e\]133;B\\e\\\\/);
  });

  test('emits OSC 133 A/C/D + OSC 7 markers', () => {
    expect(BASH_SNIPPET.snippet).toMatch(/\\e\]133;A\\e\\\\/);
    expect(BASH_SNIPPET.snippet).toMatch(/\\e\]133;C;%s\\e\\\\/);
    expect(BASH_SNIPPET.snippet).toMatch(/\\e\]133;D;%d\\e\\\\/);
    expect(BASH_SNIPPET.snippet).toMatch(/\\e\]7;file:\/\//);
  });
});

describe('FISH_SNIPPET', () => {
  test('emits OSC 133 A/C/D + OSC 7 markers', () => {
    expect(FISH_SNIPPET.snippet).toMatch(/\\e\]133;A\\e\\\\/);
    expect(FISH_SNIPPET.snippet).toMatch(/\\e\]133;C;%s\\e\\\\/);
    expect(FISH_SNIPPET.snippet).toMatch(/\\e\]133;D;%d\\e\\\\/);
    expect(FISH_SNIPPET.snippet).toMatch(/\\e\]7;file:\/\//);
  });
});

describe('shell resolution', () => {
  test.each([
    ['/bin/zsh', ZSH_SNIPPET],
    ['/usr/local/bin/zsh', ZSH_SNIPPET],
    ['/bin/bash', BASH_SNIPPET],
    ['/usr/bin/fish', FISH_SNIPPET],
  ])('getSnippetForShell(%s) returns the matching snippet', (path, expected) => {
    expect(getSnippetForShell(path)).toBe(expected);
  });

  test('getSnippetForShell returns null for unknown shells', () => {
    expect(getSnippetForShell('/bin/nu')).toBeNull();
    expect(getSnippetForShell(null)).toBeNull();
    expect(getSnippetForShell(undefined)).toBeNull();
  });

  test('shellDisplayName extracts the base name', () => {
    expect(shellDisplayName('/bin/zsh')).toBe('zsh');
    expect(shellDisplayName('/usr/local/bin/fish')).toBe('fish');
    expect(shellDisplayName(null)).toBe('your shell');
  });
});
