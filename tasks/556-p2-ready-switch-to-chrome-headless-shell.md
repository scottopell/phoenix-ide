---
id: 556
priority: p2
status: ready
title: Switch to ChromeHeadlessShell for smaller download
---

# Switch to ChromeHeadlessShell for smaller download

## Problem

Phoenix currently downloads full Chromium (~600MB) when system Chrome is unavailable. This is unnecessarily large for a headless automation use case.

Chrome for Testing provides `ChromeHeadlessShell` specifically for automation, which is ~200MB (66% smaller) and contains only what's needed for CDP automation.

## Current Behavior

```rust
// src/tools/browser/session.rs
let fetcher_opts = BrowserFetcherOptions::builder()
    .with_path(&cache_dir)
    .build()  // Uses default: BrowserKind::Chromium
```

Download sizes:
- Full Chromium: ~600 MB
- ChromeHeadlessShell: ~200 MB

Cache location: `~/.cache/phoenix-ide/chromium`

## Solution

Switch to `BrowserKind::ChromeHeadlessShell`:

```rust
let fetcher_opts = BrowserFetcherOptions::builder()
    .with_path(&cache_dir)
    .with_kind(BrowserKind::ChromeHeadlessShell)
    .build()
```

## Implementation

1. Import `BrowserKind` in `src/tools/browser/session.rs`:
   ```rust
   use chromiumoxide::fetcher::{BrowserFetcher, BrowserFetcherOptions, BrowserKind};
   ```

2. Update fetcher options in `BrowserSession::new()`:
   ```rust
   let fetcher_opts = BrowserFetcherOptions::builder()
       .with_path(&cache_dir)
       .with_kind(BrowserKind::ChromeHeadlessShell)
       .build()
   ```

3. Test all browser tools still work:
   - browser_navigate
   - browser_eval
   - browser_click
   - browser_type
   - browser_take_screenshot
   - browser_wait_for_selector
   - Console log capture

4. Run existing tests:
   ```bash
   cargo test browser
   ```

5. Optional: Clear old cache and verify download:
   ```bash
   rm -rf ~/.cache/phoenix-ide/chromium
   # Start Phoenix and trigger browser tool
   ```

## Testing

```bash
# Clear cache to force fresh download
rm -rf ~/.cache/phoenix-ide/chromium

# Run browser tests
cargo test browser

# Manual test with phoenix-client.py
./phoenix-client.py <<EOF
Navigate to example.com and take a screenshot
EOF
```

## Notes

- ChromeHeadlessShell is officially maintained by Chrome team
- Purpose-built for automation (no GUI components)
- Same CDP protocol as full Chrome
- System Chrome fallback still works (unchanged)
- First-run download will be faster (200MB vs 600MB)

## References

- chromiumoxide fetcher: `chromiumoxide_fetcher/src/kind.rs`
- Chrome for Testing: https://developer.chrome.com/blog/chrome-for-testing/
