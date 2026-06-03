export interface ResolvedFilePathCopyValues {
  absolutePath: string;
  relativePath: string;
}

function normalizeSegments(path: string): string {
  const collapsed = path.trim().replace(/\/+/g, '/');
  const absolute = collapsed.startsWith('/');
  const segments: string[] = [];

  for (const segment of collapsed.split('/')) {
    if (!segment || segment === '.') continue;
    if (segment === '..') {
      const previous = segments.at(-1);
      if (previous && previous !== '..') {
        segments.pop();
      } else if (!absolute) {
        segments.push(segment);
      }
      continue;
    }
    segments.push(segment);
  }

  const normalized = `${absolute ? '/' : ''}${segments.join('/')}`;
  if (normalized) return normalized;
  return absolute ? '/' : '';
}

export function normalizeFilePathForCopy(path: string): string {
  return normalizeSegments(path) || '/';
}

function joinRootAndRelative(rootDir: string, filePath: string): string {
  const root = normalizeFilePathForCopy(rootDir);
  const relative = normalizeSegments(filePath);
  if (!relative) return root;
  return normalizeFilePathForCopy(`${root}/${relative}`);
}

function relativePathInsideRoot(absolutePath: string, rootDir: string): string | null {
  const absolute = normalizeFilePathForCopy(absolutePath);
  const root = normalizeFilePathForCopy(rootDir);
  if (absolute === root) return '.';
  const prefix = root === '/' ? '/' : `${root}/`;
  if (!absolute.startsWith(prefix)) return null;
  return absolute.slice(prefix.length);
}

export function resolveFilePathCopyValues(displayedPath: string, rootDir: string): ResolvedFilePathCopyValues {
  const displayed = normalizeFilePathForCopy(displayedPath);
  const root = normalizeFilePathForCopy(rootDir);
  const absolutePath = displayed.startsWith('/')
    ? displayed
    : joinRootAndRelative(root, displayed);
  const relativePath = relativePathInsideRoot(absolutePath, root)
    ?? (displayed.startsWith('/') ? displayed : normalizeSegments(displayed));
  return { absolutePath, relativePath };
}
