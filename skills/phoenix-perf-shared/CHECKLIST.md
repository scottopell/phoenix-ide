# Phoenix perf-hunt suite — readiness checklist

Status: **validated as an instrument; not yet proven able to land a win.**
The suite has measured and correctly *rejected* 2 optimizations and refuted
1 static smell. It has shipped **zero accepted optimizations** — that is the
confidence gate, not a nice-to-have.

Do NOT start the PR-100 adaptation (item 8) until item 1 is done. Stay on the
`agent-browser` / `run-scenario` harness until a significant optimization has
landed end-to-end.

## Gate to "ready"

- [ ] **1. Land ≥1 successful, significant optimization end-to-end.**
      A real change that clears its threshold AND Welch p<0.05, 5/5 review
      approve, committed (not reverted). Until this exists the suite is
      proven to say "no" but unproven to say "yes". (Recorded next-angle:
      throttle react-markdown render of the OPEN streaming block — see
      db/StreamingMessage-memo-completed-blocks.yaml lessons.)

## Instrument hardening (before trusting any verdict broadly)

- [ ] **2. React-metric presence guard.** `react_commit_*` only populate
      when the React DevTools global hook exists (Vite dev). On a prod
      build they vanish and stats.py would silently run significance on the
      remaining metrics — the exact silent-wrong class PR 100's comment
      calls out. preflight + run-scenario must FAIL loudly when a scenario
      declares it needs React metrics and they are absent.
- [ ] **3. Larger-N outlier audit.** Clean at N=12 (sd≈5.7, no outliers)
      after the isolation fixes. Re-confirm at N≥30 on ≥2 scenarios; the
      first N=12 attempt had intermittent 17% contamination that small N
      hid.
- [ ] **4. Scenario coverage.** Only `sse-streaming` is proven to run.
      Validate `conversation-load` and `composer-typing` selectors +
      readiness against the live app, same as sse-streaming was.
- [ ] **5. Threshold calibration doc.** Record min-detectable-effect at the
      working N given observed noise (CV≈9% on react_commit_ms) so a
      "rejected: sub-threshold" is distinguishable from "rejected:
      underpowered".
- [ ] **6. Skill-invocation path.** The hunt was driven manually here.
      Exercise the actual `/phoenix-perf-*` Skill chain
      (preflight→find-target→hunt→review→submit) once end-to-end.

## Deferred — PR 100 / `browser_profile` adaptation (DO NOT START until 1)

- [ ] **7. Wait for PR 100 to land** (REQ-BT-019.20 page-anchored window;
      F3/F5 fix in progress on `feat/browser-profile-perf-tooling`).
- [ ] **8. Adapt harness onto `browser_profile`.** Per the PR 100 F2
      decision the tool schema is canonical and the skill owns the mapping:
      - stats.py: add adapter candidate paths `react_commits` →
        `react_commit_count`, `react_actual_ms` → `react_commit_ms`
        (the other keys already match).
      - run-scenario: swap the agent-browser transport for `browser_profile`
        while preserving the raw-sample / skill-owns-stats contract.
      - **Parity gate**: before trusting cross-harness db comparisons, run
        the same scenario on both harnesses and show equivalent baselines.
        A transport swap that shifts the baseline invalidates history.
- [ ] **9. Re-validate** items 1–4 on the new harness; the methodology is
      sound (PR 100 confirms the window method matches), but the instrument
      must be re-proven, not assumed.
