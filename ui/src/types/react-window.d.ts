declare module 'react-window' {
  import { ComponentType, CSSProperties, ReactElement } from 'react';

  export interface VariableSizeListProps {
    children: ComponentType<any>;
    height: number | string;
    itemCount: number;
    itemSize: (index: number) => number;
    width: number | string;
    itemData?: any;
    overscanCount?: number;
    onScroll?: (props: { scrollOffset: number; scrollDirection: 'forward' | 'backward' }) => void;
  }

  export class VariableSizeList extends React.Component<VariableSizeListProps> {
    scrollTo(scrollOffset: number): void;
    scrollToItem(index: number, align?: 'start' | 'end' | 'center' | 'auto'): void;
    resetAfterIndex(index: number, shouldForceUpdate?: boolean): void;
  }
}