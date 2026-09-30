import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { mobileTablePreviewCases } from './scenarios';
import { describe, expect, it } from 'vitest';
import {
  annotateTableColumnKinds,
  classifyTableColumn,
  type TableColumnKind,
} from './tableColumnKinds';

describe('mobile table preview column sizing', () => {
  it('keeps short numeric columns intrinsic and grants readable width only to prose', () => {
    expect(classifyTableColumn(['43.1 s', '84.4 s', '55.2 s', '30.2 s'])).toBe('numeric');
    expect(classifyTableColumn(['159', '138', '578', '5'])).toBe('numeric');
    expect(classifyTableColumn(['A qualified candidate needs merge/deploy, or a concrete intervention is needed'])).toBe('prose');
    expect(classifyTableColumn(['gpt-5.4-mini', 'gpt-5.6-sol'])).toBe('atomic');
    expect(classifyTableColumn(['Phoenix registry', 'Provider catalog'])).toBe('compact');
  });

  it.each(['streaming', 'final'])('annotates numeric-last and prose columns in %s rendering', () => {
    const root = document.createElement('section');
    root.innerHTML = `
      <table>
        <thead><tr><th>Model</th><th>Completed</th><th>Max</th></tr></thead>
        <tbody>
          <tr><td>gpt-5.4-mini</td><td>159</td><td>43.1 s</td></tr>
          <tr><td>gpt-5.4</td><td>138</td><td>84.4 s</td></tr>
        </tbody>
      </table>
      <table>
        <thead><tr><th>Source</th><th>Appropriate role</th></tr></thead>
        <tbody><tr><td>Phoenix registry</td><td>Models we support, their protocols, capabilities, and orchestration qualification</td></tr></tbody>
      </table>`;

    annotateTableColumnKinds(root);

    const tables = root.querySelectorAll('table');
    expect(tables[0]?.rows[0]?.cells[2]?.dataset['columnKind']).toBe('numeric');
    expect(tables[1]?.rows[0]?.cells[1]?.dataset['columnKind']).toBe('prose');
  });

  it('classifies every historical case without inflating numeric columns', () => {
    const expected: Record<string, TableColumnKind[]> = {
      'global-status': ['compact', 'prose'],
      'source-roles': ['compact', 'prose'],
      'candidate-ranking': ['numeric', 'compact', 'prose'],
      'landed-changes': ['label', 'prose'],
      'message-matrix': ['compact', 'prose', 'prose'],
      'latency-measurements': ['label', 'numeric', 'numeric'],
      'failure-policy': ['label', 'prose'],
      'store-size': ['compact', 'compact'],
      'model-performance': ['atomic', 'numeric', 'numeric', 'numeric', 'numeric', 'numeric'],
    };

    for (const previewCase of mobileTablePreviewCases) {
      const lines = previewCase.markdown.split('\n').filter((line) => line.startsWith('|'));
      const tableStart = lines.findIndex((line) => line.includes(`| ${previewCase.headers[0]} |`));
      const tableLines = lines.slice(tableStart);
      const bodyRows = tableLines.slice(2).map((line) => (
        line.slice(1, -1).split('|').map((cell) => cell.trim().replace(/[*`[\]]/g, ''))
      ));
      const columnCount = bodyRows[0]?.length ?? 0;
      const actual = Array.from({ length: columnCount }, (_, column) => (
        classifyTableColumn(bodyRows.map((row) => row[column] ?? ''))
      ));
      expect(actual, previewCase.id).toEqual(expected[previewCase.id]);
    }
  });

  it('contains no positional or blanket cell minimum-width rule', () => {
    const css = readFileSync(
      resolve(process.cwd(), 'src/fixtures/messageList/mobileTablePreview.css'),
      'utf8',
    );
    expect(css).not.toContain(':last-child');
    expect(css).not.toContain(':nth-child');
    expect(css).toContain("[data-column-kind='numeric']");
    expect(css).toContain("[data-column-kind='prose']");
    expect(css).toContain("[data-column-kind='atomic']");
    expect(css).toContain("[data-column-kind='compact']");
    const compactRule = css.match(/\[data-column-kind='compact'\][^{]*\{([^}]*)\}/)?.[1] ?? '';
    expect(compactRule).toContain('white-space: normal');
    expect(compactRule).not.toContain('white-space: nowrap');
    const atomicRule = css.match(/\[data-column-kind='atomic'\][^{]*\{([^}]*)\}/)?.[1] ?? '';
    expect(atomicRule).toContain('white-space: nowrap');
    expect(css).toContain("[data-message-list-fixture='mobile-table-preview-content-wrap'] .markdown-table-scroll table {\n  width: 100%");
  });
});
