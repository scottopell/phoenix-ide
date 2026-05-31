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

export const codeViewMockState: { scrollToCalls: unknown[]; lastItems: unknown[] } = {
  scrollToCalls: [],
  lastItems: [],
};

export function resetCodeViewMock(): void {
  codeViewMockState.scrollToCalls = [];
  codeViewMockState.lastItems = [];
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
    React.useImperativeHandle(ref, () => ({
      scrollTo: (target: unknown) => codeViewMockState.scrollToCalls.push(target),
      addItems: () => undefined,
      getItem: () => undefined,
      updateItem: () => false,
      updateItemId: () => false,
      setSelectedLines: () => undefined,
      getSelectedLines: () => null,
      clearSelectedLines: () => undefined,
      getInstance: () => undefined,
    }));

    codeViewMockState.lastItems = [...(props.items ?? [])];
    const lineProps = { annotationSide: 'additions', lineNumber: 1, lineType: 'change-addition', type: 'diff-line' };
    return (
      <div data-testid="codeview-mock" className={props.className} ref={props.containerRef}>
        {(props.items ?? []).map((item: any) => (
          <div key={item.id} data-item-id={item.id}>
            {props.renderHeaderPrefix?.(item)}
            {props.renderHeaderMetadata?.(item)}
            <span data-filename>{item.fileDiff?.name}</span>
            {(item.annotations ?? []).map((ann: any, i: number) => (
              <div key={i} data-annotation>
                {props.renderAnnotation?.(ann, item)}
              </div>
            ))}
            <button
              data-testid={`mock-line-click-${item.id}`}
              onClick={() =>
                props.options?.onLineClick?.({ ...lineProps, event: { pointerType: 'mouse' } }, { type: 'diff', item })
              }
            >
              line
            </button>
            {/* Drives Pierre's onLineEnter so the long-press handler knows the line. */}
            <button
              data-testid={`mock-line-enter-${item.id}`}
              onClick={() => props.options?.onLineEnter?.(lineProps, { type: 'diff', item })}
            >
              enter
            </button>
            {/* A touch tap: onLineClick with a touch pointer (should NOT annotate). */}
            <button
              data-testid={`mock-line-tap-${item.id}`}
              onClick={() =>
                props.options?.onLineClick?.({ ...lineProps, event: { pointerType: 'touch' } }, { type: 'diff', item })
              }
            >
              tap
            </button>
            {props.renderGutterUtility?.(() => ({ lineNumber: 1, side: 'additions' }), item)}
          </div>
        ))}
      </div>
    );
  });
  return { CodeView };
}
