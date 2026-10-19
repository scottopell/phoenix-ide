Implement the sole protected ARM packaging-controller repair for immutable candidate 237100c79d9ce672867b284167a099fa0b7a2bcf. Preserve candidate bytes and normal prepare-main/release behavior; bound stapler retries and retain separate controller provenance. Hosted signing and dispatch are outside this task.

## Validation evidence

- Final signal-propagation correction: signaled stapler delegates now raise `Interrupted(-code)` and the wrapper re-emits the actual signal, avoiding attempts to install handlers for uncatchable signals. The fake SIGKILL regression asserts subprocess return code `-SIGKILL`, one invocation, and no child output. Command timeouts remain nonretryable and return 124.
- After this correction: `python3 -m unittest discover -s tests/devpy -p test_incident_stapler_controller.py -v` passed all 18 focused tests; `bash macos/Phoenix/scripts/test-package-desktop-release.sh` passed, including retry exhaustion; `python3 scripts/test-desktop-release-workflow.py` passed.
- Parent-reported earlier validation: the full dev.py Python suite passed 1,019 tests before the latest small changes. It was not rerun after this correction. Unrelated global Allium errors in LLM files remain outside this task; no global Allium pass is claimed.
- No full build, hosted dispatch, signing, commit, or push was performed in this final scoped correction.
