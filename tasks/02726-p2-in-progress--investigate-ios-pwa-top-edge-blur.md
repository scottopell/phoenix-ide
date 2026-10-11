# Diagnose residual installed-iPhone PWA top-edge blur

Physical-iPhone acceptance found the repaired top safe-area viewport usable and the breadcrumb clickable, but a minor blur remains at the very top edge and slightly impairs readability. Investigate the actual composited boundary on the retained `fix/pwa-top-safe-area-hit-target` workstream: safe-area ownership, clipping, overlay/backdrop filters, fixed/sticky layers, and status-bar interaction. Do not remove blur globally or alter unrelated Global viewport work.

Preserve clickable controls, mobile text scaling, keyboard behavior, and safe-area ownership. Require a real-phone before/after receipt before asserting a polish fix: device model, iOS version, Safari versus installed PWA, route/build, and an uncropped screenshot plus a focused top-edge crop. If physical-device evidence is unavailable, report that gate rather than claiming acceptance from emulation.
