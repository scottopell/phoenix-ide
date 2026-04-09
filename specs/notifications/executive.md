# Notifications -- Executive Summary

## Overview

Browser desktop notifications for Phoenix IDE conversations that need
user attention. Fires on task approval, questions, errors, and agent
completion. Configurable per-event-type with server-side persistence.

## Status

| Requirement | Title | Status |
|---|---|---|
| REQ-NOTIF-001 | Browser Desktop Notifications | ❌ Not Started |
| REQ-NOTIF-002 | Notification Permission Request | ❌ Not Started |
| REQ-NOTIF-003 | Configurable Event Types | ❌ Not Started |
| REQ-NOTIF-004 | Global Scope with Per-Event Toggles | ❌ Not Started |
| REQ-NOTIF-005 | Click-to-Navigate | ❌ Not Started |
| REQ-NOTIF-006 | Notification Settings UI | ❌ Not Started |
| REQ-NOTIF-007 | SSE-Driven Notification Triggers | ❌ Not Started |

## MVP Scope

All 7 requirements are MVP. The feature is small and self-contained:
settings table + API (backend), notification hook + settings panel
(frontend). No phasing needed.

## Allium Spec

Behavioral specification: `specs/notifications/notifications.allium`
