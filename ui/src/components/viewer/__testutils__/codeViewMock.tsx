/**
 * Deterministic stand-in for `@pierre/diffs/react`'s CodeView for component
 * tests. The real CodeView tokenizes asynchronously through Shiki/worker pools
 * and renders into a virtualized `<diffs-container>`, so its line text isn't
 * synchronously queryable under happy-dom. This mock renders each item's
 * Phoenix-owned slots (header prefix/metadata, annotations, gutter utility) and
 * exposes buttons to fire the line-click and gutter callbacks, so tests can
 * exercise Phoenix's wiring without depending on Pierre's render pipeline.
 *
 * Use via:
 *   vi.mock('@pierre/diffs/react', async () => {
 *     const { makeCodeViewMock } = await import('./__testutils__/codeViewMock');
 *     return makeCodeViewMock();
 *   });
 * and read `codeViewMockState.scrollToCalls` to assert jump behavior.
 */
import React from 'react';

export const codeViewMockState: { scrollToCalls: unknown[]; lastItems: unknown[]; lastUnsafeCss: string } = {
  scrollToCalls: [],
  lastItems: [],
  lastUnsafeCss: '',
};

export function resetCodeViewMock(): void {
  codeViewMockState.scrollToCalls = [];
  codeViewMockState.lastItems = [];
  codeViewMockState.lastUnsafeCss = '';
}

/** Latest controlled `version` Pierre would reconcile against for an item. */
export function itemVersion(id: string): number | undefined {
  const item = codeViewMockState.lastItems.find(
    (i): i is { id: string; version?: number } =>
      typeof i === 'object' && i !== null && (i as { id?: string }).id === id,
  );
  return item?.version;
}

// The mock intentionally mirrors only the surface PhoenixDiffCodeView touches;
// `any` keeps it from re-stating Pierre's full generic prop types.
/* eslint-disable @typescript-eslint/no-explicit-any */
export function makeCodeViewMock() {
  const CodeView = React.forwardRef(function CodeViewMock(props: any, ref: any) {
    // Per-item container elements, keyed by id, so getRenderedItems() can hand
    // back the same { element, item } shape the real CodeView exposes — the
    // touch resolver matches a pointer's composed path against these.
    const itemEls = React.useRef(new Map<string, HTMLElement>());

    React.useImperativeHandle(ref, () => ({
      scrollTo: (target: unknown) => codeViewMockState.scrollToCalls.push(target),
      addItems: () => undefined,
      getItem: () => undefined,
      updateItem: () => false,
      updateItemId: () => false,
      setSelectedLines: () => undefined,
      getSelectedLines: () => null,
      clearSelectedLines: () => undefined,
      getInstance: () => ({
        getRenderedItems: () =>
          (props.items ?? [])
            .filter((it: any) => itemEls.current.has(it.id))
            .map((it: any) => ({
              id: it.id,
              type: it.type ?? 'diff',
              item: it,
              version: it.version,
              element: itemEls.current.get(it.id),
              instance: {},
            })),
      }),
    }));

    codeViewMockState.lastItems = [...(props.items ?? [])];
    codeViewMockState.lastUnsafeCss = props.options?.unsafeCSS ?? '';
    return (
      <div data-testid="codeview-mock" className={props.className} ref={props.containerRef}>
        {(props.items ?? []).map((item: any) => {
          // The mock mirrors both Pierre item shapes: a diff item carries a
          // side per line, a file item is sideless. Tests for either viewer fire
          // the same callbacks Phoenix wires.
          const isFile = item.type === 'file';
          const lineProps = isFile
            ? { lineNumber: 1, type: 'line' }
            : { annotationSide: 'additions', lineNumber: 1, lineType: 'change-addition', type: 'diff-line' };
          const ctx = { type: isFile ? 'file' : 'diff', item };
          const hovered = isFile ? { lineNumber: 1 } : { lineNumber: 1, side: 'additions' };
          return (
            <div
              key={item.id}
              data-item-id={item.id}
              ref={(el: HTMLElement | null) => {
                if (el) itemEls.current.set(item.id, el);
                else itemEls.current.delete(item.id);
              }}
            >
              <div data-testid={`mock-header-${item.id}`}>
                {props.renderHeaderPrefix?.(item)}
                {props.renderHeaderMetadata?.(item)}
                <span data-filename>{isFile ? item.file?.name : item.fileDiff?.name}</span>
              </div>
              {/* Pierre-like line DOM (data attributes the touch resolver reads). */}
              <div data-code="" data-additions="">
                <span data-line="1" data-line-type="change-addition" data-testid={`mock-line-el-${item.id}`}>
                  line text
                </span>
              </div>
              {(item.annotations ?? []).map((ann: any, i: number) => (
                <div key={i} data-annotation>
                  {props.renderAnnotation?.(ann, item)}
                </div>
              ))}
              <button
                data-testid={`mock-line-click-${item.id}`}
                onClick={() =>
                  props.options?.onLineClick?.({ ...lineProps, event: { pointerType: 'mouse' } }, ctx)
                }
              >
                line
              </button>
              {/* A touch tap: onLineClick with a touch pointer (should NOT annotate). */}
              <button
                data-testid={`mock-line-tap-${item.id}`}
                onClick={() =>
                  props.options?.onLineClick?.({ ...lineProps, event: { pointerType: 'touch' } }, ctx)
                }
              >
                tap
              </button>
              {props.renderGutterUtility?.(() => hovered, item)}
            </div>
          );
        })}
      </div>
    );
  });
  return { CodeView };
}
