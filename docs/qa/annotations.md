# Annotation design review

Annotation states are direct Ladle stories using the production conversation,
reaction pill, draft cards, source highlights, and composer. The fixtures seed
their stores and use production navigation and send actions to reach each state.
No selection, typing, or submission is needed to inspect a story.

## Capture the comparison gallery

```sh
./dev.py qa annotations
```

Requires `phoenix-browser` on PATH. The command reuses the Ladle server on
port 61123 or starts a temporary one. It captures fourteen states at desktop
1440×900 and mobile 390×844, in light and dark themes, using the browser CLI.
Each story reports readiness after its target state and layout settle.
Unexpected browser errors and horizontal document overflow fail the capture;
the failed-send story permits its intentional synthetic rejection.

The output is ignored by Git:

- `ui/qa-artifacts/annotations/index.html` — side-by-side gallery with
  theme/state filters, full-resolution images, live story links, and composer
  height as pixels and a percentage of the viewport.
- `ui/qa-artifacts/annotations/*.png` — fifty-six screenshots.
- `ui/qa-artifacts/annotations/captures.json` — capture measurements.

Open the HTML file directly, or keep Ladle running with `pnpm --dir ui ladle`
and use the `Open in Ladle` URL printed by the capture command. This uses
Vite’s development-only `/@fs/` route to serve the local gallery and images.
Captures live outside `public/` so production builds do not copy them into app assets.

For a focused design edit, regenerate only the relevant states:

```sh
ANNOTATION_THEMES=light \
CAPTURE_STORIES=05-one-annotation,06-several-annotations \
./dev.py qa annotations
```

That produces four screenshots and a smaller gallery. Run the unfiltered
command again to restore the complete gallery.

## Direct stories

Ladle groups these under **Annotations**:

1. `01-empty` — ordinary draft, no annotations.
2. `02-selected` — selected source with empty reaction input.
3. `03-writing` — multiline reaction entry.
4. `04-discard` — inline Keep/Discard confirmation.
5. `05-one-annotation` — one compact annotation strip.
6. `06-several-annotations` — three annotations.
7. `07-long-comment` — a long comment collapsed to the same compact preview.
8. `08-sending` — in-flight annotation with editing disabled.
9. `09-failed-send` — editable annotation and restored draft after rejection.
10. `10-sent` — submitted message, cleared annotations and highlights.
11. `11-two-annotations` — intermediate stack for spatial comparison with one and three.
12. `12-editing` — second annotation selected, full quotation and long comment in the modal.
13. `13-after-delete` — middle annotation deleted; surviving labels stay 1 and 3.
14. `14-many-annotations` — eight annotations in the bounded scrolling stack.

For example:
<http://127.0.0.1:61123/?story=annotations--06-several-annotations&mode=preview&fixtureTheme=light>

Change production styles/components, regenerate the captures, and refresh the
gallery to compare screen usage. Browser emulation supplies design snapshots;
native phone selection and keyboard acceptance remain separate manual checks.
