# Reserve the installed iOS PWA top safe area for clickable header controls

## User report
Human supplied an iPhone installed-PWA screenshot on 2026-09-27 during the catalog ENOSPC recovery coordination. Top conversation title, Recall, Auto-continue and Work controls appear faintly blurred beneath the iOS top system region; user reports insufficient reserved top space makes controls unclickable. The screenshot is in the coordinator conversation alongside this request. This is a reported hit-target/layout defect; exact CSS cause is not yet verified.

## Scope
Investigate and fix the narrow top safe-area layout boundary for the installed web PWA. Check viewport-fit/status-bar metadata, env(safe-area-inset-top), app shell/header positioning, scrolling and visual viewport interactions before choosing a fix. Avoid device-specific guessed padding, double insets, a shell rewrite, or changes to unrelated reaction-dock behavior. Related queued web-layout task 24711 covers broader installed-iPhone/iPad shell contracts; this report is a narrow concrete defect within that area, not authorization to implement the entire shell program. Reconcile ownership and scope against 24711 before admission.

## Acceptance
- Header controls and complete hit targets stay below the iOS protected top region in the installed PWA.
- Check initial load, conversation navigation, scrolling, rotation and keyboard open/close; desktop and ordinary Safari retain correct layout without duplicate top spacing.
- Add regression coverage at the owning layout boundary and record installed-iPhone PWA validation separately from desktop viewport emulation. Do not claim physical validation without a real receipt.
- Preserve compact usable screen space; use the correct safe-area/layout source rather than an arbitrary per-device offset.

## Coordination
User commissioned this narrow fix. Queued behind urgent catalog delivery; no implementation owner admitted yet. Before admission, deduplicate against any active PWA/safe-area owner. Do not automatically wake the parked mobile dock owner or paused flake stream. Global retains merge/deploy ownership for coordinated delivery.


## Physical acceptance gate

Automated desktop/emulated viewport evidence is not physical-device acceptance. Before merge, serve the normal Vite production artifact through the existing authenticated preview bridge and install that preview on a real notched iPhone.

Record a receipt containing device model, iOS version, installed-PWA URL/build commit, and screenshots or video for:

1. Cold launch from the Home Screen: title and every top control render and hit entirely below the protected system region.
2. Navigate list → conversation → back and repeat top-control taps.
3. Scroll transcript to both ends; verify the shell inset remains fixed and is not consumed by transcript scrolling.
4. Rotate portrait ↔ landscape; verify the browser-updated `safe-area-inset-top` is used without duplicate spacing.
5. Focus and dismiss the composer keyboard; verify the top inset neither disappears nor doubles.
6. Open the same build in ordinary Safari; verify no standalone-only top padding is applied.

Production artifact path for this worktree after `corepack pnpm build`:
`/Users/sopell/git/phoenix-ide/.phoenix/coordinator-worktrees/pwa-top-safe-area-hit-target/ui/dist/index.html`

Physical acceptance: **pending**.
