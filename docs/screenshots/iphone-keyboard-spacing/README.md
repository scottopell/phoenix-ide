# Standalone composer: bounded synthetic QA

## Scope and source

Base: `7b0de589029bd842dab794599a3a69f77481fa3f`. The product change only moves
an existing mobile standalone composer bottom-padding exception after its general
safe-area default. The earlier exception lost in the cascade. Horizontal insets,
viewport ownership, autosize and StateBar are unchanged.

Approved two-file product/test patch SHA-256:
`f904b8046924c7af5f8a89a852ea60a6ad1e7f9085e540c50ff078477f17c2c4`.
The owner reports two independent reviews plus owner review with no findings.
This record is synthetic evidence, not physical-device acceptance.

## Results

36 matched before/after samples (72 observations) used cached Chromium
149.0.7827.55 and WebKit 26.5 with Playwright 1.61.0:

- Each engine: browser/standalone stylesheet paths × bottom inset 0/34 ×
  closed 852 / reduced 500 / restored 852 CSS-pixel heights at width 393,
  focused empty draft (12 samples); standalone inset 34 additionally used a
  three-line draft at width 393 and capped overlong draft at width 320
  across all three heights (6 samples).
- All 18 standalone/inset-34 samples: composer-to-StateBar separator middle
  band **40 → 6px**, recovering 34px. Browser/inset-34 stays 40px;
  every zero-inset middle band stays 6px.
- StateBar primary controls stay **44px** tall; standalone outer bottom inset
  stays **34px**. Neither is removed by this fix. The StateBar touches the
  visual viewport bottom; composer and StateBar do not overlap.
- Focus/draft and closed-to-restored geometry retained; no document scroll,
  reported runtime errors, blocked requests or composer horizontal overflow.
  Transcript tail remaining distance is 0–1px. The tail-to-composer band is not
  targeted: one WebKit narrow/capped reduced sample varies 25.5 → 26.5px;
  all other paired tail bands match. No transcript-spacing fix is claimed.

`measurements.json` is a path-free numeric extract from the original before/after
reports, including report hashes. Publication rechecked pair identity, all middle
bands, unchanged StateBar controls/bottom band, focus, document scroll and tail
remaining distance. It does not replace the full local harness or independently
re-run browser measurements.

## Selected synthetic captures

All four images were visually inspected before publication. They contain only
deterministic fixture messages, not the private original image or production text.
Each is 393 × 500 CSS pixels at DPR 3, focused empty draft, standalone stylesheet
path, bottom inset 34. No keyboard is present.

| Engine | Before | After |
| --- | --- | --- |
| Chromium | [Synthetic before](chromium-before-synthetic.png) | [Synthetic after](chromium-after-synthetic.png) |
| WebKit | [Synthetic before](webkit-before-synthetic.png) | [Synthetic after](webkit-after-synthetic.png) |

## Validation and reproduction boundaries

The retained local `composer-fix/RESULT.md`, before/after reports and logs record:
new regression red against original CSS (23 pass / 1 fail), then **24/24** viewport
regressions green; full cached UI ESLint and Stylelint pass; app and node TypeScript
projects pass. These checks were performed before publication; no local dependency
install, Rust/native build or full repository check was performed for publication.
With the repository's normal dependencies already available, the committed focused
regression can be rerun from `ui` with
`pnpm exec vitest run src/components/DesktopLayout.viewport.test.tsx`.

The actual-shell fixture mounts DesktopLayout and the production
route/provider/transcript/composer chain, but its mocked API, fixture extensions,
stories, capture scripts and exploratory runner are **uncommitted local/private
harness work, excluded from this PR**. Accordingly this checkout alone cannot
reproduce the screenshot matrix. No broad helper churn or dependency changes ship.

Chromium used a native CDP safe-area environment override. Its native display-mode
emulation was accepted but ignored: standalone used **TEST-ONLY in-place media
clause substitution**, preserving selectors/source order. WebKit used
**TEST-ONLY env substitution**, plus the same standalone media substitution.
JavaScript standalone detection stayed false. Browser controls used native browser
media. Reduced layout and visual viewports changed together; this is **not** an
independent visualViewport keyboard shrink, real iPhone, installed PWA, Safari
chrome, WKWebView or native-client test.

The original report was clarified as Home Screen PWA. Physical PWA/keyboard
acceptance remains unverified; task 36024 stays in progress. No merge, deployment,
production-phone action or conclusion about the original private image is implied.
The owned fixture server was stopped and owned `ui/node_modules` symlink removed;
borrowed dependencies and all preexisting unrelated dirty assets were preserved.
