import { MessageSquarePlus } from 'lucide-react';
import { useLongPress } from '../../hooks/useLongPress';

/**
 * A single annotatable unit of viewer content — a source line, a code line, or
 * a rendered markdown block. Long-press (touch) or the inline button opens the
 * annotation dialog for that line. Shared by every text-like viewer body so the
 * long-press semantics and the modified/highlighted styling live in one place.
 */
export interface AnnotatableBlockProps {
  as?: React.ElementType;
  lineNumber: number;
  lineContent: string;
  onAnnotate: (lineNumber: number, lineContent: string) => void;
  isModified?: boolean;
  isHighlighted?: boolean;
  lineRef?: (el: HTMLElement | null) => void;
  className?: string;
  children?: React.ReactNode;
  [key: string]: unknown;
}

export function AnnotatableBlock({
  as: Tag = 'div',
  lineNumber,
  lineContent,
  onAnnotate,
  isModified,
  isHighlighted,
  lineRef,
  className,
  children,
  ...rest
}: AnnotatableBlockProps) {
  const lp = useLongPress<{ lineNumber: number; lineContent: string }>(
    ({ lineNumber: ln, lineContent: lc }) => onAnnotate(ln, lc),
  );
  const cls = [
    'annotatable',
    className,
    isModified && 'annotatable--modified',
    isHighlighted && 'annotatable--highlighted',
  ].filter(Boolean).join(' ');

  return (
    <Tag
      ref={(el: HTMLElement | null) => lineRef?.(el)}
      className={cls}
      onTouchStart={(e: React.TouchEvent) => lp.start(e, { lineNumber, lineContent })}
      onTouchMove={lp.move}
      onTouchEnd={lp.end}
      onMouseDown={(e: React.MouseEvent) => lp.start(e, { lineNumber, lineContent })}
      onMouseMove={lp.move}
      onMouseUp={lp.end}
      onMouseLeave={lp.end}
      data-line={lineNumber}
      {...rest}
    >
      {children}
      <button
        className="annotatable__btn"
        onClick={(e: React.MouseEvent) => {
          e.stopPropagation();
          onAnnotate(lineNumber, lineContent);
        }}
        aria-label={`Add note to line ${lineNumber}`}
        title="Add note"
      >
        <MessageSquarePlus size={14} />
      </button>
    </Tag>
  );
}

/**
 * Props every text-like viewer body receives from MetaViewer. The body owns
 * rendering and registers each line's DOM node via `registerLineRef`; MetaViewer
 * owns the ref map (for jump-to-line) and the annotation/notes lifecycle.
 */
export interface ViewerBodyProps {
  content: string;
  modifiedLines: Set<number>;
  highlightedLine: number | null;
  onAnnotate: (lineNumber: number, lineContent: string) => void;
  registerLineRef: (lineNumber: number, el: HTMLElement | null) => void;
}
