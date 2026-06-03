import { describe, expect, it } from 'vitest';
import { resolveFilePathCopyValues } from './filePathCopy';

describe('resolveFilePathCopyValues', () => {
  it('resolves a relative path under root', () => {
    expect(resolveFilePathCopyValues('src/main.rs', '/repo/project')).toEqual({
      absolutePath: '/repo/project/src/main.rs',
      relativePath: 'src/main.rs',
    });
  });

  it('makes an absolute path under root relative to root', () => {
    expect(resolveFilePathCopyValues('/repo/project/src/main.rs', '/repo/project')).toEqual({
      absolutePath: '/repo/project/src/main.rs',
      relativePath: 'src/main.rs',
    });
  });

  it('keeps an absolute path outside root as the relative fallback', () => {
    expect(resolveFilePathCopyValues('/other/project/src/main.rs', '/repo/project')).toEqual({
      absolutePath: '/other/project/src/main.rs',
      relativePath: '/other/project/src/main.rs',
    });
  });

  it('handles root trailing slashes and duplicate path slashes', () => {
    expect(resolveFilePathCopyValues('src//main.rs', '/repo/project/')).toEqual({
      absolutePath: '/repo/project/src/main.rs',
      relativePath: 'src/main.rs',
    });
  });

  it('resolves dot segments in relative paths against root', () => {
    expect(resolveFilePathCopyValues('../shared/util.ts', '/repo/project/packages/app')).toEqual({
      absolutePath: '/repo/project/packages/shared/util.ts',
      relativePath: '../shared/util.ts',
    });
  });
});
