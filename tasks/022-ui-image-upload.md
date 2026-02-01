---
created: 2026-01-31
priority: p3
status: ready
---

# Add Image Upload to Web UI

## Summary

Allow users to attach images to messages via file picker, paste, or camera capture.

## Context

The backend accepts base64-encoded images in chat messages (REQ-API-004). The `read_image` tool exists for the agent to read images. Users need a way to share screenshots, photos, and diagrams.

## Acceptance Criteria

- [ ] File picker button to select images
- [ ] Paste support (Cmd/Ctrl+V) for clipboard images
- [ ] Preview thumbnails of attached images before sending
- [ ] Remove button on each thumbnail
- [ ] Images encoded as base64 with correct media_type
- [ ] Mobile: camera capture option via `capture="environment"` attribute
- [ ] Supported formats: PNG, JPEG, GIF, WebP

## Notes

- See `api.sendMessage()` in `static/app.js` - already accepts images array
- Backend expects `{data: base64string, media_type: "image/png"}`
- Consider drag-and-drop support for desktop
- May want to resize large images client-side to reduce payload
