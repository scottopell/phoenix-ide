---
created: 2026-02-09
priority: p3
status: ready
---

# Browser Tool Enhancements

## Summary

Follow-up improvements for the new browser_click, browser_type, and browser_wait_for_selector tools.

## Potential Enhancements

### browser_click
- [ ] Support clicking by text content (e.g., `{"text": "Submit"}`)
- [ ] Add `double_click` option
- [ ] Add `right_click` option for context menus
- [ ] Handle elements covered by overlays (scroll into view, wait for overlay to disappear)

### browser_type
- [ ] Support special keys beyond Enter (Tab, Escape, Arrow keys)
- [ ] Add `delay` option for realistic typing speed
- [ ] Support key combinations (Ctrl+A, Cmd+V)

### browser_wait_for_selector
- [ ] Add `hidden` option (wait for element to disappear)
- [ ] Add `count` option (wait for N elements matching selector)
- [ ] Return element info (text content, attributes) when found

### New Tools to Consider
- [ ] `browser_select` - Select dropdown option by value/text
- [ ] `browser_hover` - Hover over element (for tooltips, menus)
- [ ] `browser_scroll` - Scroll to element or coordinates
- [ ] `browser_wait_for_navigation` - Wait for page load after click
- [ ] `browser_get_text` - Get text content of element (simpler than eval)
- [ ] `browser_get_attribute` - Get element attribute value

## Notes

These are nice-to-haves. The current tools cover 90% of use cases. browser_eval remains available for edge cases.
