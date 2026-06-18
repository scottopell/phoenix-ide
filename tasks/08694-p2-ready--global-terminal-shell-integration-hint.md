The global terminal (WorkScope::Global) mounts TerminalPanel without `shell`, `homeDir`, or an assist-setup callback on both the /new pane and the dedicated /terminal route. Only the per-conversation terminal (ConversationPage) passes these. As a result, when the global shell lacks OSC 133 / OSC 7 shell integration, the absent-integration hint cannot render the real zsh/bash/fish snippet and the "Let Phoenix set this up for me" CTA is disabled (`canAssist` requires shell + homeDir + onAssistSetup).

Fix: source the server shell/home from deployment info and pass them to the global TerminalPanel on both surfaces so the snippet renders. Optionally wire a global-scope assist-setup callback that seeds a new conversation in $HOME so the full setup CTA works.

Surfaced by Codex review on the /terminal PR; pre-existing for /new too.
