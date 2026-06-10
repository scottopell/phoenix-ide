# Vendored React UMD bundles (browser-tool test fixtures)

Pinned production UMD builds used by the browser-tool React-profiling tests,
served inline from the test's local server so the tests have **no off-box
network dependency** (they previously fetched these from unpkg.com, which made
them flake when the CDN was slow/unreachable).

- `react-18.3.1.production.min.js`        — react@18.3.1 `umd/react.production.min.js`
- `scheduler-0.23.2.production.min.js`     — scheduler@0.23.2 `umd/scheduler.production.min.js`
- `react-dom-18.3.1.production.min.js`     — react-dom@18.3.1 `umd/react-dom.production.min.js`
- `react-dom-18.3.1.profiling.min.js`      — react-dom@18.3.1 `umd/react-dom.profiling.min.js`

Source: the corresponding npm packages (identical bytes to `ui/node_modules`).
To refresh, recopy the matching `umd/*.{production,profiling}.min.js` for the pinned version.
