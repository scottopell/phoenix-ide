Phoenix logs JSON to stdout; the phoenix.log (dev) and ~/.phoenix-ide/prod.log (prod) files are shell/systemd redirections set up outside the binary, so the running process cannot truthfully report "the log file path."

Introduce a PHOENIX_LOG_FILE env var that the binary honors (tee/append tracing output to that file in addition to or instead of stdout), so the deployment can report an authoritative log path on the About-this-deployment page (REQ-DEPLOY-006).

Until then, the deployment page ships a placeholder: it reports the log file path only when explicitly configured, otherwise states logs go to stdout captured by the supervisor. Do not claim the dev/prod redirection conventions as if the binary owned them.

See specs/deployment-info/requirements.md REQ-DEPLOY-006.
