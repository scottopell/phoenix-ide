# Viewer Slot Requirements

The slot kinds and transitions are defined in [viewer_slot.allium](viewer_slot.allium). These requirements refine the URL boundary without changing those transitions.

## REQ-VS-002: Unified URL Slot Contract

WHEN a viewer-only transition opens, closes, changes presentation, restores, or normalizes the viewer slot,
THE SYSTEM SHALL replace the current history entry's viewer parameters while preserving the pathname, non-viewer query parameters, hash, and router location state.

THE SYSTEM SHALL preserve the underlying transcript reader's navigation identity across viewer-only replacements so opening or dismissing a viewer does not replay a message anchor, chapter selection, or source-navigation command.

WHEN an actual reader navigation occurs through push, replace, or history traversal,
THE SYSTEM SHALL retain its navigation-command semantics, including hash, source-transcript, and chapter selection, even when the destination repeats an earlier target.

WHILE a concurrent render is uncommitted,
THE SYSTEM SHALL NOT publish its location or reader navigation identity as authority for viewer mutations or navigation-command consumption.

## REQ-VS-004: User Closes Active Viewer

WHEN a supported viewer dismissal closes the active slot,
THE SYSTEM SHALL clear only viewer-owned URL parameters and preserve the underlying reader state under REQ-VS-002.

This applies to shared viewer close controls and Escape handling where dismissal is supported; it does not require outside-click dismissal or override find, annotation, or pending-feedback guards.
