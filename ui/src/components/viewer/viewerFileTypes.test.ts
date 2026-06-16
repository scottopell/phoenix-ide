import { describe, it, expect } from 'vitest';
import { classifyViewerFile } from './viewerFileTypes';

describe('classifyViewerFile — extension fallback (no server type)', () => {
  it('classifies markdown', () => {
    expect(classifyViewerFile('README.md')).toEqual({ renderKind: 'markdown', language: 'markdown' });
    expect(classifyViewerFile('notes.markdown').renderKind).toBe('markdown');
  });

  it('classifies html with a source/preview-capable kind', () => {
    expect(classifyViewerFile('index.html')).toEqual({ renderKind: 'html', language: 'html' });
    expect(classifyViewerFile('page.htm').renderKind).toBe('html');
  });

  it('classifies code with its highlighter language', () => {
    expect(classifyViewerFile('main.rs')).toEqual({ renderKind: 'code', language: 'rust' });
    expect(classifyViewerFile('app.tsx')).toEqual({ renderKind: 'code', language: 'tsx' });
    // config-ish extensions highlight as code in the extension fallback,
    // matching the pre-dedup ProseReader behaviour.
    expect(classifyViewerFile('config.json')).toEqual({ renderKind: 'code', language: 'json' });
    expect(classifyViewerFile('Cargo.toml')).toEqual({ renderKind: 'code', language: 'toml' });
  });

  it('classifies unknown / extensionless / plain text as text', () => {
    expect(classifyViewerFile('notes.txt')).toEqual({ renderKind: 'text', language: 'text' });
    expect(classifyViewerFile('LICENSE')).toEqual({ renderKind: 'text', language: 'text' });
    expect(classifyViewerFile('data.weirdext')).toEqual({ renderKind: 'text', language: 'text' });
  });

  it('handles paths with directories and multiple dots', () => {
    expect(classifyViewerFile('/a/b/c/main.test.ts').renderKind).toBe('code');
    expect(classifyViewerFile('/a/b/c/main.test.ts').language).toBe('typescript');
  });
});

describe('classifyViewerFile — server TextCategory as authority', () => {
  it('trusts the server category for the markdown/code/plain split', () => {
    // Server says config; render highlights as code.
    expect(classifyViewerFile('settings.ini', 'config').renderKind).toBe('code');
    // Server says plain for .log; render as plain lines.
    expect(classifyViewerFile('server.log', 'plain').renderKind).toBe('text');
    // Server's "unknown" → plain text, never code.
    expect(classifyViewerFile('Makefile', 'unknown').renderKind).toBe('text');
    expect(classifyViewerFile('README.md', 'markdown').renderKind).toBe('markdown');
  });

  it('still applies the html override even when the server bucketed it as code', () => {
    // Server lumps .html into "code"; the renderer needs the html split for
    // the source/preview toggle.
    expect(classifyViewerFile('index.html', 'code')).toEqual({ renderKind: 'html', language: 'html' });
  });

  it('keeps the extension-derived language regardless of server category', () => {
    expect(classifyViewerFile('main.rs', 'code').language).toBe('rust');
    expect(classifyViewerFile('q.graphql', 'config').language).toBe('graphql');
  });
});
