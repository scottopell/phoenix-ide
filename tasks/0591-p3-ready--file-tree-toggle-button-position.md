---
created: 2026-02-28
number: 591
priority: p3
status: ready
slug: file-tree-toggle-button-position
title: "File tree drawer toggle button jumps between top and bottom on collapse/expand"
---

# File Tree Toggle Button Position

## Problem

The file tree panel on desktop has an open/close drawer button that moves from the top
of the screen when expanded to the bottom when collapsed (or vice versa). The toggle
button should stay in a consistent position regardless of drawer state so users can
double-click to toggle without moving the cursor.

## Fix

Pin the toggle button to a consistent position (top of the panel area) in both expanded
and collapsed states. The button should not move when the drawer opens or closes.

## Files

- Look in `ui/src/components/` for the file explorer / file tree panel component
- CSS in `ui/src/index.css` for the panel layout
