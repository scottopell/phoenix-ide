# Consolidate browser DOM-eval fixture

Run compatible deterministic DOM-eval regression scenarios through one panic-safe serialized browser fixture instead of separate Chrome sessions, while preserving each expression's named semantic assertion and isolation from session/network/profile contracts. Capture launch counts, targeted and warmed-lane CPU/wall measurements, deliberate fault proof, broad validation, and a separate PR.


## Evidence

Five direct test-harness samples measured the three original exact tests at summed medians of 4,200 ms CPU and 3,140 ms wall across three Chrome launches. The shared fixture measured 1,410 ms CPU and 1,090 ms wall across one launch, saving 2,790 ms CPU (66.43%), 2,050 ms wall (65.29%), and two launches (66.67%).

Each original expression remains explicit with its own diagnostic: `document.body.innerText`, `document.body.innerHTML.slice(0, 200)`, and the bug-report `JSON.stringify({bodyText: document.body.innerText})` shape. Deliberate faults breaking each expression/path independently all failed the shared fixture; logs are under `target/qa-evidence/53010/faults/`. The broader browser suite passed 49/49. Separate complex-page, await, promise, syntax-error, pre-navigation, profile, network, and ownership contracts remain isolated.
