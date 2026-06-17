Vitest flake: happy-dom iframe loads hit the real network

## Symptom

`./dev.py check` (and therefore `./dev.py prod deploy`, which gates on a full
`cmd_check(gate=False)`) intermittently failed locally with:

    FAIL src/components/MessageList.test.tsx > renders skill invocations …
    Error: Test timed out in 5000ms.

Always green in CI, intermittent locally. The failing test is a *synchronous*
render with no async work, so a 5s timeout could only mean it never got
scheduled — it was starved, not hung.

## Root cause

`MetaViewer` / `HtmlViewerBody` render a sandboxed
`<iframe className="viewer-iframe" src="/preview/…">`. `MetaViewer.test.tsx`
mounts it with `src="/preview/tmp/project/thing"`.

happy-dom resolves that relative src against the default document base
(`http://localhost:3000/`) and *actually loads the iframe page* — a real
network fetch. The smoking gun was in the suite stderr:

    DOMException [NetworkError]: Failed to execute "fetch()" on "Window"
    with URL "http://localhost:3000/preview/tmp/project/thing":
    The operation was aborted.
    … happy-dom AsyncTaskManager.destroy

Why local-only and intermittent:
- In CI nothing listens on :3000 → connection refused → the fetch rejects
  instantly → no hang.
- Locally something accepts the connection on :3000 (a Vite/worktree dev
  server or proxy) and holds it open → the iframe load becomes a live async
  task that hangs → happy-dom's AsyncTaskManager keeps the worker busy →
  co-scheduled synchronous tests (MessageList being the unlucky one) get
  starved past the 5s timeout.

So MessageList was the victim, not the cause. Any test sharing the worker pool
under load could have tipped over.

## Fix

Disable iframe page loading in the happy-dom test environment globally, so
tests never touch the network for iframe content:

    // ui/vitest.config.ts
    test: {
      environment: 'happy-dom',
      environmentOptions: {
        happyDOM: { settings: { disableIframePageLoading: true } },
      },
    }

The iframe src now rejects synchronously (`NotSupportedError: … Iframe page
loading is disabled.`) instead of hanging. Confirmed across 3 consecutive full
runs: 993/993 passing, no NetworkError/AsyncTaskManager noise, no timeout.

## Lesson

Tests must never depend on real network behaviour, including implicit resource
loads (iframes, img src, link/script tags). A test that passes or fails based
on whether a port happens to be listening is environment-coupled. happy-dom
will faithfully attempt these loads unless told not to.
