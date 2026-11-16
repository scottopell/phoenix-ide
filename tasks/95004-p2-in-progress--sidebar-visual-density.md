# Sidebar visual density and badge allocation

Compact the existing sidebar typography and spacing; allocate title, state, PR badge and actions without overlap. Preserve focus, unread/selection and touch targets. Qualify synthetic desktop and narrow/touch before/after via hosted captures; no deploy or local native build. Reference the populated second user screenshot privately.

## Implementation and qualification

- PR #1094: compact 13px titles, shrinkable title allocation, reserved PR/menu targets, narrow badge reflow, compact header/Ember/tabs and explicit 44px coarse-pointer targets. Product TSX change is only the Sidebar stylesheet import.
- Merged main through hydration PR #1095 (`e74907481402ddce1a837a08488dc2eeb8866d20`) via Git; preserved hydration/count/availability semantics.
- Passing combined-source visual run: https://github.com/scottopell/phoenix-ide/actions/runs/43796705673 at `5c0d285e82c9982dab7aa796b0358836dfaf7973`. Separate historic RC6 CSS, before and after phases; normal/narrow desktop/touch and continued-product actions. Strict sweep covers each integer220–240 plus160/180/200/260/280/320/500.
- Strict testing found touch title under-allocation at230px; independent review found active-row border cost. Final reflow threshold238px list width gives the active merged/unread title40px minimum at239px pane width. Fixtures assert active+unread rather than hiding the selection border.
- Independent source/render and measurement/harness audits found no remaining visual blocker. Settings content/viewport/overflow/bottom-action hit-testing, keyboard row/actions and touch targets qualify in the hosted harness. External exact-head review requested; responses must be distinguished from empty review arrays.
- Durable synthetic-only evidence: `docs/screenshots/sidebar-visual-density/README.md`. RC6 is historic CSS on current fixture DOM, not full historic-app or production-deployment proof. No physical-device claim.
- Broad CI exposed a fixture presentation sum-type access and Ember stylesheet ordering. Fixed by preserving the typed presentation wholesale, adding a needs-action regression, and ordering existing declarations without changing values. Sidebar QA now uses `tsc -b` (root `tsc --noEmit` did not check referenced projects). Final-head hosted checks pending after these corrections. Local full tsc is additionally blocked by missing existing ajv dependency; no install attempted.
- No merge/deployment/shared restart performed. Integration authority remains to be verified before merge; task stays in-progress until final checks and handoff gates close.
