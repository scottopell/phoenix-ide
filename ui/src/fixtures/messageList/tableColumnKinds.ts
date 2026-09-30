export type TableColumnKind = 'numeric' | 'atomic' | 'compact' | 'prose' | 'label';
const SHORT_ATOMIC_CODE = /^\S{1,12}$/;

const NUMERIC_VALUE = /^[+−-]?(?:\d[\d,.]*)(?:\s*[–—-]\s*\d[\d,.]*)?\s*(?:%|ms|s|KiB|MiB|GiB|rows?)?$/i;

export function classifyTableColumn(values: string[]): TableColumnKind {
  const normalized = values.map((value) => value.trim()).filter(Boolean);
  if (normalized.length > 0 && normalized.every((value) => NUMERIC_VALUE.test(value))) {
    return 'numeric';
  }
  if (normalized.every((value) => !/\s/.test(value) && value.length <= 24)) {
    return 'atomic';
  }
  if (normalized.every((value) => {
    const words = value.split(/\s+/).filter(Boolean);
    return words.length <= 4 && value.length <= 28;
  })) {
    return 'compact';
  }
  if (normalized.some((value) => {
    const words = value.split(/\s+/).filter(Boolean);
    return words.length >= 8;
  })) {
    return 'prose';
  }
  return 'label';
}

export function annotateTableColumnKinds(root: ParentNode): void {
  for (const code of root.querySelectorAll('code')) {
    const value = code.textContent?.trim() ?? '';
    if (SHORT_ATOMIC_CODE.test(value)) {
      code.dataset['tokenKind'] = 'short-atomic';
    } else {
      code.dataset['tokenKind'] = 'breakable';
    }
  }
  for (const table of root.querySelectorAll('table')) {
    const rows = [...table.rows];
    const columnCount = Math.max(0, ...rows.map((row) => row.cells.length));
    for (let column = 0; column < columnCount; column += 1) {
      const bodyValues = rows.slice(1).flatMap((row) => {
        const cell = row.cells.item(column);
        return cell ? [cell.textContent ?? ''] : [];
      });
      const kind = classifyTableColumn(bodyValues);
      for (const row of rows) {
        const cell = row.cells.item(column);
        if (cell && cell.dataset['columnKind'] !== kind) {
          cell.dataset['columnKind'] = kind;
        }
      }
    }
  }
}
