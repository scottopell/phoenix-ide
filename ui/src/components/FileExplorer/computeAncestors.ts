/**
 * Compute the chain of ancestor directories between `rootPath` (exclusive)
 * and `activeFile` (exclusive). Returns absolute paths in root-to-leaf order.
 *
 * Returns [] in two distinct cases:
 *   1. activeFile sits directly at the root (no intermediate directories
 *      to expand) — use {@link isUnderRoot} to disambiguate.
 *   2. activeFile is not under rootPath at all.
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
  const root = normalizeRoot(rootPath);
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

/**
 * True iff `activeFile` is `rootPath` itself or a descendant of it. Strictly
 * structural — no filesystem access, no symlink resolution. Lets callers
 * distinguish "file at root, nothing to expand" from "file outside root,
 * don't even try."
 */
export function isUnderRoot(rootPath: string, activeFile: string): boolean {
  const root = normalizeRoot(rootPath);
  return activeFile === root || activeFile.startsWith(root + '/');
}

function normalizeRoot(rootPath: string): string {
  return rootPath.endsWith('/') ? rootPath.slice(0, -1) : rootPath;
}
