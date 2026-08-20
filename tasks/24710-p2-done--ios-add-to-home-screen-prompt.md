# Add iOS Home Screen identity and standalone viewport support

## User journey

A user adds Phoenix to an iPhone or iPad Home Screen using Safari's native Share → Add to Home Screen action. The installed web app should have a deliberate Phoenix icon and name, open without Safari chrome, use the larger standalone viewport correctly, and respect notches, the Dynamic Island, and the Home indicator without wasting space in ordinary Safari.

Phoenix does not provide installation onboarding. The user already knows how to invoke the native iOS action.

## Requirements

### Home Screen identity

- Provide a web app manifest with Phoenix name/short name, application start URL and scope, standalone display mode, theme/background colors, and purpose-built raster icons at appropriate install sizes.
- Provide a dedicated 180×180 Apple touch icon. Do not rely on the current small SVG favicon for iOS Home Screen rendering.
- Add Apple-compatible standalone-capable, title, and status-bar metadata while retaining standards-based manifest metadata.
- Keep icon artwork legible under iOS masking. The source asset must include deliberate background and optical padding rather than placing important artwork against the image edge.
- Ensure all metadata and assets are emitted in production builds and available through Phoenix's embedded static UI.

### Standalone viewport and safe areas

- Retain `viewport-fit=cover` so the standalone app can use the full screen.
- Treat the standalone viewport as larger than Safari's browser viewport because browser chrome is absent. Use responsive layout and `100dvh`; do not encode viewport heights from a specific device.
- Audit the root app shell, mobile conversation-list header/list/footer, conversation header/composer, dialogs, and overlays at all four edges.
- Apply `env(safe-area-inset-*)` only at the surface that owns each physical screen edge. Do not stack safe-area padding in nested components or add fixed global margins.
- Content and controls must not sit beneath the status area, notch/Dynamic Island, rounded screen corners, or Home indicator.
- Ordinary in-browser Safari must not gain unnecessary standalone-only whitespace.
- Theme/background colors must avoid a visible flash or mismatched strip around the standalone app shell.

### Explicit exclusions

- No install prompt, onboarding card, Settings entry, or synthetic Install button.
- No service worker, offline behavior, cache lifecycle, update prompt, or offline claim. Phoenix may remain an online-only installable web app.
- No ProductConversation or conversation-list presentation redesign.

## Validation

- Validate manifest shape, referenced assets, raster dimensions, HTML metadata, and production-build output.
- Add focused regression coverage for any standalone-mode detection or standalone-only style policy introduced.
- Browser/fixture QA at representative notched iPhone and iPad dimensions in both ordinary-browser and standalone display modes.
- Verify top and bottom controls remain usable with simulated non-zero safe-area insets and that browser mode does not receive duplicate padding.
- Run focused UI checks and relevant `./dev.py check` lanes.
