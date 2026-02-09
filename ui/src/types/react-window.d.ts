declare module 'react-window' {
  import { ComponentType } from 'react';

  export interface ListChildComponentProps {
    index: number;
    style: React.CSSProperties;
    data: unknown;
  }

  export interface VariableSizeListProps {
    children: ComponentType<ListChildComponentProps>;
    height: number | string;
    itemCount: number;
    itemSize: (index: number) => number;
    width: number | string;
    itemData?: unknown;
    overscanCount?: number;
    onScroll?: (props: { scrollOffset: number; scrollDirection: 'forward' | 'backward' }) => void;
  }

  export class VariableSizeList extends React.Component<VariableSizeListProps> {
    scrollTo(scrollOffset: number): void;
    scrollToItem(index: number, align?: 'start' | 'end' | 'center' | 'auto'): void;
    resetAfterIndex(index: number, shouldForceUpdate?: boolean): void;
  }
}
