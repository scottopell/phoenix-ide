Phoenix logs JSON to stdout; the phoenix.log (dev) and ~/.phoenix-ide/prod.log (prod) files are shell/systemd redirections set up outside the binary, so the running process cannot truthfully report "the log file path."

Introduce a PHOENIX_LOG_FILE env var that the binary honors — wire the tracing subscriber to tee/append output to that file (in addition to or instead of stdout) — so the deployment genuinely owns a log file it can report on the About-this-deployment page (REQ-DEPLOY-006).

The logger wiring and the reported sink must land in the SAME change, from a single source of truth: the deployment's `LogInfo` is derived from what the logger actually writes, NOT from the presence of the env var. Today `build_deployment_config` hardcodes `LogInfo::Stdout` because the logger only writes stdout; reporting `LogInfo::File` from the env var alone would point operators at a file the process never opens. When the logger is wired, construct `LogInfo::File { path }` at the same place the file appender is configured.

Implementation note: `api/deployment.rs::LogInfo::File` carries an `#[expect(dead_code)]` attribute (it is currently unconstructed). Constructing it as part of this task will make the `expect` fire — remove the attribute then.

See specs/deployment-info/requirements.md REQ-DEPLOY-006 and design.md ("The log sink reflects what the logger does").
