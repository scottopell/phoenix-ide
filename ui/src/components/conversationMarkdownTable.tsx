import React from 'react';
import { classifyTableColumn, reactNodeText, type TableColumnKind } from './conversationMarkdownTableSemantics';

type MarkdownTableProps = React.ComponentPropsWithoutRef<'table'> & { node?: unknown };
type ElementWithChildren = React.ReactElement<{ children?: React.ReactNode }>;


function tableBodyRows(children: React.ReactNode): string[][] {
  const rows: string[][] = [];
  const visit = (node: React.ReactNode, inBody: boolean): void => {
    React.Children.forEach(node, (child) => {
      if (!React.isValidElement<{ children?: React.ReactNode }>(child)) return;
      const element = child as ElementWithChildren;
      const childInBody = inBody || element.type === 'tbody';
      if (element.type === 'tr' && childInBody) {
        const cells = React.Children.toArray(element.props.children)
          .filter((cell): cell is ElementWithChildren => React.isValidElement(cell) && cell.type === 'td')
          .map((cell) => reactNodeText(cell.props.children));
        if (cells.length > 0) rows.push(cells);
        return;
      }
      visit(element.props.children, childInBody);
    });
  };
  visit(children, false);
  return rows;
}

function annotateCells(node: React.ReactNode, kinds: TableColumnKind[]): React.ReactNode {
  return React.Children.map(node, (child) => {
    if (!React.isValidElement<{ children?: React.ReactNode }>(child)) return child;
    const element = child as ElementWithChildren;
    if (element.type === 'tr') {
      let column = 0;
      const cells = React.Children.map(element.props.children, (cell) => {
        if (!React.isValidElement<{ children?: React.ReactNode }>(cell)
          || (cell.type !== 'th' && cell.type !== 'td')) return cell;
        const kind = kinds[column] ?? 'label';
        column += 1;
        return React.cloneElement(cell, { 'data-column-kind': kind } as React.HTMLAttributes<HTMLElement>);
      });
      return React.cloneElement(element, undefined, cells);
    }
    return React.cloneElement(element, undefined, annotateCells(element.props.children, kinds));
  });
}

export function ConversationMarkdownTable({ node, children, ...props }: MarkdownTableProps) {
  void node;
  const rows = tableBodyRows(children);
  const columnCount = Math.max(0, ...rows.map((row) => row.length));
  const kinds = Array.from({ length: columnCount }, (_, column) => (
    classifyTableColumn(rows.map((row) => row[column] ?? ''))
  ));

  return (
    <div className="markdown-table-scroll">
      <table {...props}>{annotateCells(children, kinds)}</table>
    </div>
  );
}
