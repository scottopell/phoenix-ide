# Browser Automation Tool - Executive Summary

## Requirements Summary

The Phoenix Browser Tool enables AI agents to interact with web pages during development, testing, and debugging. It addresses three user stories: web development (navigate, screenshot, debug), automated testing (interact via JavaScript, capture evidence), and specialized PWA testing (service workers, offline mode).

The MVP covers 90% of use cases with six core capabilities: navigation with error handling, JavaScript execution for universal page interaction, screenshot capture with LLM vision support, console log access for debugging, viewport resizing for responsive testing, and image file reading. An implicit session model eliminates browser lifecycle management burden from agents.

Post-MVP extends to PWA-specific needs: service worker inspection, network source identification, offline simulation, and multi-context console capture.

## Technical Summary

Built using `chromiumoxide` crate for async CDP communication. Tools are stateless, receiving all context via `ToolContext`. The `ctx.browser()` method provides correct-by-construction session access - conversation ID is derived internally, making it impossible to use wrong session.

`BrowserSessionManager` (owned by Runtime) maps conversation IDs to Chrome instances. Sessions auto-start on first `browser()` call, auto-cleanup after 30-minute idle timeout. Cleanup hooks fire on conversation delete and server shutdown.

Core tools wrap chromiumoxide's Page API: navigation, JavaScript evaluation, screenshots, viewport control. Console logs captured via CDP event subscription. Large outputs redirect to files. Screenshots resized for LLM vision limits.

## Status Summary

### MVP Requirements

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BT-001:** Navigate to URLs | ❌ Not Started | - |
| **REQ-BT-002:** Execute JavaScript | ❌ Not Started | - |
| **REQ-BT-003:** Take Screenshots | ❌ Not Started | - |
| **REQ-BT-004:** Capture Console Logs | ❌ Not Started | - |
| **REQ-BT-005:** Resize Viewport | ❌ Not Started | - |
| **REQ-BT-006:** Read Image Files | ✅ Complete | Existing `read_image` tool |
| **REQ-BT-010:** Implicit Session Model | ❌ Not Started | BrowserSessionManager |
| **REQ-BT-011:** State Persistence | ❌ Not Started | Session guard pattern |
| **REQ-BT-012:** Stateless Tools with Context | 🔄 In Progress | ToolContext refactor (shared with bash) |

### Post-MVP Requirements

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BT-020:** Service Worker Inspection | ❌ Not Started | PWA-specific |
| **REQ-BT-021:** Network Request Source | ❌ Not Started | PWA-specific |
| **REQ-BT-022:** Offline Mode Simulation | ❌ Not Started | PWA-specific |
| **REQ-BT-023:** Multi-Context Console | ❌ Not Started | PWA-specific |
| **REQ-BT-024:** Capture Network Requests | ❌ Not Started | API debugging |

**MVP Progress:** 1 of 9 complete (REQ-BT-006 exists)  
**Total Progress:** 1 of 14 complete
