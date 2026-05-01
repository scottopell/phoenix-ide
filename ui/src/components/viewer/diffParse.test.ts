import { describe, it, expect } from 'vitest';
import { parseUnifiedDiff } from './diffParse';

describe('parseUnifiedDiff', () => {
  it('returns empty for empty input', () => {
    expect(parseUnifiedDiff('')).toEqual([]);
  });

  it('parses a single new-file segment', () => {
    const diff = [
      'diff --git a/foo.rs b/foo.rs',
      'new file mode 100644',
      'index 0000000..abc1234',
      '--- /dev/null',
      '+++ b/foo.rs',
      '@@ -0,0 +1,2 @@',
      '+fn main() {}',
      '+',
    ].join('\n');
    const segs = parseUnifiedDiff(diff);
    expect(segs).toHaveLength(1);
    expect(segs[0]?.filePath).toBe('foo.rs');
    expect(segs[0]?.binary).toBe(false);
    const adds = segs[0]?.lines.filter((l) => l.kind === 'add') ?? [];
    expect(adds).toHaveLength(2);
    expect(adds[0]?.newLine).toBe(1);
    expect(adds[1]?.newLine).toBe(2);
  });

  it('tracks new/old line numbers across context + add + del', () => {
    const diff = [
      'diff --git a/x.rs b/x.rs',
      'index aaa..bbb 100644',
      '--- a/x.rs',
      '+++ b/x.rs',
      '@@ -10,4 +10,5 @@',
      ' context-a',
      '-removed',
      '+added-1',
      '+added-2',
      ' context-b',
    ].join('\n');
    const segs = parseUnifiedDiff(diff);
    const lines = segs[0]?.lines ?? [];
    const ctxA = lines.find((l) => l.text === ' context-a');
    const removed = lines.find((l) => l.text === '-removed');
    const added1 = lines.find((l) => l.text === '+added-1');
    const ctxB = lines.find((l) => l.text === ' context-b');
    expect(ctxA?.oldLine).toBe(10);
    expect(ctxA?.newLine).toBe(10);
    expect(removed?.oldLine).toBe(11);
    // Removed lines have no new-line number.
    expect(removed?.newLine).toBeUndefined();
    expect(added1?.newLine).toBe(11);
    // After 1 removed + 2 added, context resumes at old=12, new=13.
    expect(ctxB?.oldLine).toBe(12);
    expect(ctxB?.newLine).toBe(13);
  });

  it('splits multi-file diff into separate segments', () => {
    const diff = [
      'diff --git a/a.txt b/a.txt',
      '--- a/a.txt',
      '+++ b/a.txt',
      '@@ -1 +1 @@',
      '-old',
      '+new',
      'diff --git a/b.txt b/b.txt',
      '--- a/b.txt',
      '+++ b/b.txt',
      '@@ -1 +1 @@',
      '-x',
      '+y',
    ].join('\n');
    const segs = parseUnifiedDiff(diff);
    expect(segs).toHaveLength(2);
    expect(segs[0]?.filePath).toBe('a.txt');
    expect(segs[1]?.filePath).toBe('b.txt');
  });

  it('handles renames via `rename to` header', () => {
    const diff = [
      'diff --git a/old/path.rs b/new/path.rs',
      'similarity index 95%',
      'rename from old/path.rs',
      'rename to new/path.rs',
      '--- a/old/path.rs',
      '+++ b/new/path.rs',
      '@@ -1 +1 @@',
      '-x',
      '+y',
    ].join('\n');
    const segs = parseUnifiedDiff(diff);
    expect(segs[0]?.filePath).toBe('new/path.rs');
    expect(segs[0]?.oldPath).toBe('old/path.rs');
  });

  it('marks binary segments', () => {
    const diff = [
      'diff --git a/img.png b/img.png',
      'index aaa..bbb 100644',
      'Binary files a/img.png and b/img.png differ',
    ].join('\n');
    const segs = parseUnifiedDiff(diff);
    expect(segs[0]?.binary).toBe(true);
  });

  it('assigns stable diffPos based on line index', () => {
    const diff = [
      'diff --git a/x b/x',
      '@@ -1 +1 @@',
      '+a',
    ].join('\n');
    const segs = parseUnifiedDiff(diff);
    const lines = segs[0]?.lines ?? [];
    expect(lines[0]?.diffPos).toBe(0);
    expect(lines[1]?.diffPos).toBe(1);
    expect(lines[2]?.diffPos).toBe(2);
  });

  it('preserves \\ No newline at end of file marker', () => {
    const diff = [
      'diff --git a/x b/x',
      '--- a/x',
      '+++ b/x',
      '@@ -1 +1 @@',
      '-old',
      '\\ No newline at end of file',
      '+new',
    ].join('\n');
    const segs = parseUnifiedDiff(diff);
    const noNewline = segs[0]?.lines.find((l) => l.kind === 'no-newline');
    expect(noNewline).toBeDefined();
  });
});
