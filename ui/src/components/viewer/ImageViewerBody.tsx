import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { flushSync } from 'react-dom';
import type { PointerEvent, MouseEvent } from 'react';

const VIEW_PADDING = 24;
const MAX_SCALE = 8;
const ZOOM_SENSITIVITY = 0.006;

type Size = { width: number; height: number };
type Point = { x: number; y: number };

/**
 * Image body — renders an image inside an interactive viewer surface. Images are
 * not annotatable, so this carries no review-note wiring.
 */
export function ImageViewerBody({ fileName, url, viewKey }: { fileName: string; url: string; viewKey: string }) {
  const surfaceRef = useRef<HTMLDivElement>(null);
  const pendingScrollRef = useRef<Point | null>(null);
  const scaleRef = useRef<number | null>(null);
  const naturalSizeRef = useRef<Size | null>(null);
  const dragRef = useRef<{ pointerId: number; start: Point; scroll: Point; moved: boolean } | null>(null);
  const suppressNextClickRef = useRef(false);

  const [naturalSize, setNaturalSize] = useState<Size | null>(null);
  const [surfaceSize, setSurfaceSize] = useState<Size>({ width: 0, height: 0 });
  const [scale, setScale] = useState<number | null>(null);
  const [isDragging, setIsDragging] = useState(false);

  useEffect(() => {
    naturalSizeRef.current = null;
    setNaturalSize(null);
  }, [url]);

  const measureSurface = useCallback(() => {
    const surface = surfaceRef.current;
    if (!surface) return;
    const rect = surface.getBoundingClientRect();
    setSurfaceSize({ width: rect.width, height: rect.height });
  }, []);

  useEffect(() => {
    scaleRef.current = null;
    setScale(null);
    pendingScrollRef.current = null;
    const surface = surfaceRef.current;
    if (surface) {
      surface.scrollLeft = 0;
      surface.scrollTop = 0;
    }
    requestAnimationFrame(measureSurface);
  }, [url, viewKey, measureSurface]);

  useLayoutEffect(() => {
    measureSurface();
    const surface = surfaceRef.current;
    if (!surface) return undefined;

    const observer = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(measureSurface) : null;
    observer?.observe(surface);
    window.addEventListener('resize', measureSurface);
    return () => {
      observer?.disconnect();
      window.removeEventListener('resize', measureSurface);
    };
  }, [measureSurface, viewKey]);

  const fitScale = naturalSize ? getFitScale(naturalSize, surfaceSize) : 1;
  const currentScale = scale ?? fitScale;
  const scaledSize = naturalSize
    ? { width: naturalSize.width * currentScale, height: naturalSize.height * currentScale }
    : { width: 0, height: 0 };
  const canvasSize = {
    width: Math.max(surfaceSize.width, scaledSize.width + VIEW_PADDING * 2),
    height: Math.max(surfaceSize.height, scaledSize.height + VIEW_PADDING * 2),
  };
  const zoomed = currentScale > fitScale + 0.001;

  useLayoutEffect(() => {
    const pending = pendingScrollRef.current;
    const surface = surfaceRef.current;
    if (!pending || !surface) return;
    surface.scrollLeft = pending.x;
    surface.scrollTop = pending.y;
    pendingScrollRef.current = null;
  }, [currentScale, canvasSize.width, canvasSize.height]);

  const resetView = useCallback(() => {
    scaleRef.current = null;
    flushSync(() => setScale(null));
    const surface = surfaceRef.current;
    if (surface) {
      surface.scrollLeft = 0;
      surface.scrollTop = 0;
    } else {
      pendingScrollRef.current = { x: 0, y: 0 };
    }
  }, []);

  const zoomToScaleAt = useCallback((clientX: number, clientY: number, nextScaleTarget: number) => {
    const surface = surfaceRef.current;
    const activeNaturalSize = naturalSizeRef.current;
    if (!surface || !activeNaturalSize) return;

    const rect = surface.getBoundingClientRect();
    const pointer = { x: clientX - rect.left, y: clientY - rect.top };
    const activeFitScale = getFitScale(activeNaturalSize, { width: rect.width, height: rect.height });
    const activeScale = scaleRef.current ?? activeFitScale;
    const activeScaledSize = {
      width: activeNaturalSize.width * activeScale,
      height: activeNaturalSize.height * activeScale,
    };
    const activeCanvasSize = {
      width: Math.max(rect.width, activeScaledSize.width + VIEW_PADDING * 2),
      height: Math.max(rect.height, activeScaledSize.height + VIEW_PADDING * 2),
    };
    const activeImageInset = {
      x: (activeCanvasSize.width - activeScaledSize.width) / 2,
      y: (activeCanvasSize.height - activeScaledSize.height) / 2,
    };

    const focalImagePoint = {
      x: clamp((surface.scrollLeft + pointer.x - activeImageInset.x) / activeScale, 0, activeNaturalSize.width),
      y: clamp((surface.scrollTop + pointer.y - activeImageInset.y) / activeScale, 0, activeNaturalSize.height),
    };

    const nextScale = clamp(nextScaleTarget, activeFitScale, MAX_SCALE);
    const nextScaledSize = {
      width: activeNaturalSize.width * nextScale,
      height: activeNaturalSize.height * nextScale,
    };
    const nextCanvasSize = {
      width: Math.max(rect.width, nextScaledSize.width + VIEW_PADDING * 2),
      height: Math.max(rect.height, nextScaledSize.height + VIEW_PADDING * 2),
    };
    const nextImageInset = {
      x: (nextCanvasSize.width - nextScaledSize.width) / 2,
      y: (nextCanvasSize.height - nextScaledSize.height) / 2,
    };

    const targetScroll = nextScale === activeFitScale
      ? { x: 0, y: 0 }
      : {
          x: focalImagePoint.x * nextScale + nextImageInset.x - pointer.x,
          y: focalImagePoint.y * nextScale + nextImageInset.y - pointer.y,
        };

    scaleRef.current = nextScale === activeFitScale ? null : nextScale;
    flushSync(() => setScale(scaleRef.current));
    surface.scrollLeft = targetScroll.x;
    surface.scrollTop = targetScroll.y;
  }, []);

  const zoomAt = useCallback((clientX: number, clientY: number, deltaY: number) => {
    const surface = surfaceRef.current;
    const activeNaturalSize = naturalSizeRef.current;
    if (!surface || !activeNaturalSize) return;

    const rect = surface.getBoundingClientRect();
    const activeFitScale = getFitScale(activeNaturalSize, { width: rect.width, height: rect.height });
    const activeScale = scaleRef.current ?? activeFitScale;
    zoomToScaleAt(clientX, clientY, activeScale * Math.exp(-deltaY * ZOOM_SENSITIVITY));
  }, [zoomToScaleAt]);

  const handleClick = (event: MouseEvent<HTMLDivElement>) => {
    if (suppressNextClickRef.current) {
      suppressNextClickRef.current = false;
      return;
    }
    if (zoomed) {
      resetView();
    } else if (fitScale < 1) {
      zoomToScaleAt(event.clientX, event.clientY, 1);
    }
  };

  const handleZoomWheel = useCallback((event: Pick<WheelEvent, 'ctrlKey' | 'clientX' | 'clientY' | 'deltaY' | 'preventDefault' | 'stopPropagation'>) => {
    if (!event.ctrlKey) return;
    event.preventDefault();
    event.stopPropagation();
    zoomAt(event.clientX, event.clientY, event.deltaY);
  }, [zoomAt]);

  useEffect(() => {
    const surface = surfaceRef.current;
    if (!surface) return undefined;

    surface.addEventListener('wheel', handleZoomWheel, { passive: false });
    return () => surface.removeEventListener('wheel', handleZoomWheel);
  }, [handleZoomWheel]);

  const handlePointerDown = (event: PointerEvent<HTMLDivElement>) => {
    const surface = surfaceRef.current;
    if (!surface || !zoomed) return;
    event.preventDefault();
    dragRef.current = {
      pointerId: event.pointerId,
      start: { x: event.clientX, y: event.clientY },
      scroll: { x: surface.scrollLeft, y: surface.scrollTop },
      moved: false,
    };
    setIsDragging(true);
    event.currentTarget.setPointerCapture?.(event.pointerId);
  };

  const handlePointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const surface = surfaceRef.current;
    const drag = dragRef.current;
    if (!surface || !drag || drag.pointerId !== event.pointerId) return;
    event.preventDefault();
    const dx = event.clientX - drag.start.x;
    const dy = event.clientY - drag.start.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
    surface.scrollLeft = drag.scroll.x - dx;
    surface.scrollTop = drag.scroll.y - dy;
  };

  const stopDragging = (event: PointerEvent<HTMLDivElement>) => {
    if (dragRef.current?.pointerId === event.pointerId) {
      suppressNextClickRef.current = dragRef.current.moved;
      event.currentTarget.releasePointerCapture?.(event.pointerId);
      dragRef.current = null;
      setIsDragging(false);
    }
  };

  const clickZoomAvailable = !zoomed && fitScale < 1;
  const zoomPercent = Math.round(currentScale * 100);

  return (
    <div className="image-preview-frame">
      <div className="image-preview-toolbar" aria-live="polite">
        <span>{zoomPercent}%</span>
        {zoomed && (
          <button type="button" className="image-preview-reset" onClick={resetView}>
            Reset
          </button>
        )}
      </div>
      <div
        ref={surfaceRef}
        className={`image-preview ${zoomed ? 'image-preview--zoomed' : ''} ${isDragging ? 'image-preview--dragging' : ''} ${clickZoomAvailable ? 'image-preview--click-zoom-in' : ''}`}
        data-testid="image-preview-surface"
        onClick={handleClick}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={stopDragging}
        onPointerCancel={stopDragging}
      >
        <div
          className="image-preview-canvas"
          style={{ width: canvasSize.width || '100%', height: canvasSize.height || '100%' }}
        >
          <img
            src={url}
            alt={fileName}
            className="image-preview-img"
            draggable={false}
            width={scaledSize.width || undefined}
            height={scaledSize.height || undefined}
            onLoad={(event) => {
              const nextNaturalSize = {
                width: event.currentTarget.naturalWidth,
                height: event.currentTarget.naturalHeight,
              };
              naturalSizeRef.current = nextNaturalSize;
              setNaturalSize(nextNaturalSize);
            }}
          />
        </div>
      </div>
    </div>
  );
}

function getFitScale(naturalSize: Size, surfaceSize: Size) {
  if (naturalSize.width <= 0 || naturalSize.height <= 0 || surfaceSize.width <= 0 || surfaceSize.height <= 0) {
    return 1;
  }
  const availableWidth = Math.max(1, surfaceSize.width - VIEW_PADDING * 2);
  const availableHeight = Math.max(1, surfaceSize.height - VIEW_PADDING * 2);
  return Math.min(1, availableWidth / naturalSize.width, availableHeight / naturalSize.height);
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}
