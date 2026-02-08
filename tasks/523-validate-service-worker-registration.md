---
created: 2026-02-04
priority: p1
status: pending
assignee: human-required
---

# Validate Service Worker Auto-Registration

## Summary

During automated testing, the service worker registration in `main.tsx` did not trigger automatically on page load. Manual registration worked correctly, but the automatic registration via `window.addEventListener('load', ...)` may have a timing issue.

## Context

The service worker registration code exists in the built JavaScript bundle and can be triggered manually. However, it doesn't appear to register automatically when the page loads. This could be due to the load event firing before the async registration function adds its event listener.

## Validation Steps

1. Start Phoenix IDE: `./dev.py up`
2. Open Chrome and navigate to http://localhost:8000
3. Open DevTools → Application → Service Workers
4. Clear any existing service workers for localhost:8000
5. Hard refresh the page (Ctrl+Shift+R)
6. Check if a service worker automatically registers

## Expected Behavior

- Service worker should automatically register on page load
- Should see "[SW] Registration successful" in console
- DevTools should show active service worker for http://localhost:8000

## Actual Behavior (from automated testing)

- No automatic registration observed
- Manual registration via `navigator.serviceWorker.register('/service-worker.js')` works correctly
- No console logs from service worker registration on page load

## Potential Fixes

If auto-registration is not working:

1. Move registration outside async function to ensure load listener is added synchronously
2. Check if registration is already complete before adding load listener
3. Consider using DOMContentLoaded instead of load event

## Why Human Agent Required

LLM agents cannot currently:
- Access Chrome DevTools Application panel
- View service worker registration status
- Clear browser service workers
- Verify console logs from service worker context

This requires manual browser DevTools interaction.
