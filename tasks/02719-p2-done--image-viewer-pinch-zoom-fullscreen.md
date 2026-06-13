# Add trackpad image zoom and fullscreen for MetaViewer

## Goal

Make image payloads in the MetaViewer feel like an image viewer rather than a static `<img>`:

- MacBook trackpad pinch should zoom the image itself, not the whole page.
- Panning should be smooth when zoomed.
- Provide an easy fullscreen/focused viewing option from the viewer header.
- Keep existing text/code/html viewer behavior unchanged.

## Current implementation

`MetaViewer` routes `kind: 'image'` to `ImageViewerBody`, which currently renders only:

```tsx
<div className="image-preview">
  <img src={url} alt={fileName} className="image-preview-img" />
</div>
```

The surrounding `.viewer-content` scroll container allows normal browser gestures to bubble, so a trackpad pinch triggers browser/page zoom instead of an image-local zoom interaction.

## Implementation plan

1. Replace `ImageViewerBody` with an interactive image viewer component:
   - Track local `scale`, pan offset, and gesture anchor state.
   - Intercept trackpad pinch gestures on the image viewer surface (`wheel` with `ctrlKey` on Chromium/macOS) and `preventDefault()` so the page does not zoom.
   - Zoom around the cursor/gesture focal point rather than only the image center.
   - Clamp zoom to sensible bounds, e.g. fit/1x minimum and a practical max such as 8x.
   - Allow drag-to-pan when zoomed.
   - Support double-click or a toolbar button to reset to fit/100%.

2. Add image-only header controls through `MetaViewer`:
   - A fullscreen/takeover toggle for image payloads.
   - Optional compact zoom controls if they are useful and do not duplicate gesture behavior.
   - Reuse the existing `ViewerShell` `mode="takeover"` styling rather than inventing a parallel modal.

3. Update CSS:
   - Make `.image-preview` a contained gesture surface (`touch-action: none`/overscroll containment as appropriate).
   - Ensure transformed images render crisply and do not affect layout size.
   - Preserve current fit-to-panel behavior at the default zoom.
   - Ensure inline MetaViewer layouts still work.

4. Tests:
   - Extend `MetaViewer.test.tsx` or add `ImageViewerBody.test.tsx` for image routing plus the new controls.
   - Verify the fullscreen button appears for image payloads and changes viewer mode/label/state.
   - Verify a synthetic pinch-like wheel event calls `preventDefault()` and updates transform/zoom state.
   - Verify non-image payloads do not get image controls.

## Acceptance criteria

- In the MetaViewer image panel on macOS trackpads, pinch zoom affects the image and does not zoom the whole page.
- Zoom is smooth enough for practical inspection and stays within defined min/max bounds.
- A zoomed image can be panned without accidentally scrolling the app shell.
- Fullscreen/focused mode is available for images and can be exited reliably with the existing close/back affordance and Escape behavior.
- Text, code, markdown, and HTML viewer payloads are not regressed.
