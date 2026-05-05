/**
 * Compute the chain of ancestor directories between `rootPath` (exclusive)
 * and `activeFile` (exclusive). Returns absolute paths in root-to-leaf order.
 *
 * Returns [] if `activeFile` is not under `rootPath`, or if it sits directly
 * at the root (no intermediate directories to expand).
 *
 * Example:
 *   rootPath   = /home/u/proj
 *   activeFile = /home/u/proj/ui/src/x.tsx
 *   →          [/home/u/proj/ui, /home/u/proj/ui/src]
 *
 * Lives in its own module so FileTree.tsx can stay a pure-component file
 * (eslint react-refresh/only-export-components).
 */
export function computeAncestors(rootPath: string, activeFile: string): string[] {
  const root = rootPath.endsWith('/') ? rootPath.slice(0, -1) : rootPath;
  const prefix = root + '/';
  if (!activeFile.startsWith(prefix)) return [];
  const rel = activeFile.slice(prefix.length);
  const parts = rel.split('/').filter(Boolean);
  // parts.length === 1 → file sits directly under root, no ancestors to expand.
  if (parts.length <= 1) return [];
  const ancestors: string[] = [];
  let acc = root;
  for (let i = 0; i < parts.length - 1; i++) {
    acc = acc + '/' + parts[i];
    ancestors.push(acc);
  }
  return ancestors;
}
