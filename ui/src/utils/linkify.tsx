/**
 * Text linkification utility
 * 
 * Parses text to find URLs and file paths, rendering them as clickable elements.
 * File paths open in the prose reader when clicked.
 */

import React from 'react';

// Regex for matching URLs (http:// and https://)
// Avoids matching trailing punctuation that's likely not part of the URL
// eslint-disable-next-line no-useless-escape
const URL_REGEX = /https?:\/\/[^\s<>"'`\]\)]+[^\s<>"'`\]\).,:;!?]/g;

// File extension whitelist for relative/project paths (where we need the extension
// to avoid false positives). Absolute paths starting with / don't need this.
const FILE_EXTENSIONS = 'md|markdown|rs|ts|tsx|js|jsx|py|go|json|yaml|yml|toml|txt|css|scss|html|htm|vue|svelte|sh|bash|sql|graphql|proto|xml|ini|env|conf|cfg|lock|c|h|cpp|hpp|java|kt|swift|rb|php|ex|exs|hs|ml|zig|scala|mod|sum';

// Regex for matching file paths that look like real files
// Two strategies:
//   1. Absolute paths (/...): Match any multi-segment path. The leading / is strong
//      enough signal — no extension required. Catches Dockerfile, Makefile, .gitignore,
//      go.mod, directories, etc.
//   2. Relative/project paths (./foo, src/foo): Require a recognized extension to
//      avoid false positives on plain words containing slashes.
// Note: Uses lookbehind for proper word boundary detection
const FILE_PATH_REGEX = new RegExp(
  '(?:^|(?<=[\\s`"\'(\\[]))' +
  '(?:' +
    // Absolute paths: /foo/bar (at least two segments, no extension required)
    '(?:\\/[\\w.-]+(?:\\/[\\w.-]+)+)' +
    '|' +
    // Relative paths with extension: ./file.ext, ../dir/file.ext
    '(?:\\.\\.\\/[\\w./-]+|\\.\\/[\\w./-]+)\\.(?:' + FILE_EXTENSIONS + ')' +
    '|' +
    // Project paths with extension: src/api/mod.rs
    '(?:[\\w.-]+\\/[\\w./-]+)\\.(?:' + FILE_EXTENSIONS + ')' +
  ')' +
  '(?=[\\s`"\'\\)\\],:;!?]|$)',
  'g'
);

export interface LinkifySegment {
  type: 'text' | 'link' | 'file';
  content: string;
  href: string | undefined;
  filePath: string | undefined;
}

interface MatchInfo {
  index: number;
  length: number;
  type: 'link' | 'file';
  content: string;
  href?: string;
  filePath?: string;
}

/**
 * Parse text and extract URLs and file paths as separate segments.
 */
export function parseLinks(text: string): LinkifySegment[] {
  const matches: MatchInfo[] = [];

  // Find all URL matches
  URL_REGEX.lastIndex = 0;
  let match;
  while ((match = URL_REGEX.exec(text)) !== null) {
    matches.push({
      index: match.index,
      length: match[0].length,
      type: 'link',
      content: match[0],
      href: match[0],
    });
  }

  // Find all file path matches
  FILE_PATH_REGEX.lastIndex = 0;
  while ((match = FILE_PATH_REGEX.exec(text)) !== null) {
    // Skip if this overlaps with a URL match
    const overlaps = matches.some(
      (m) => match!.index < m.index + m.length && match!.index + match![0].length > m.index
    );
    if (!overlaps) {
      matches.push({
        index: match.index,
        length: match[0].length,
        type: 'file',
        content: match[0],
        filePath: match[0],
      });
    }
  }

  // Sort matches by index
  matches.sort((a, b) => a.index - b.index);

  // Build results
  const results: LinkifySegment[] = [];
  let lastIndex = 0;

  for (const m of matches) {
    // Add text before the match
    if (m.index > lastIndex) {
      results.push({
        type: 'text',
        content: text.slice(lastIndex, m.index),
        href: undefined,
        filePath: undefined,
      });
    }

    // Add the match
    if (m.type === 'link') {
      results.push({
        type: 'link',
        content: m.content,
        href: m.href,
        filePath: undefined,
      });
    } else {
      results.push({
        type: 'file',
        content: m.content,
        href: undefined,
        filePath: m.filePath,
      });
    }

    lastIndex = m.index + m.length;
  }

  // Add remaining text after last match
  if (lastIndex < text.length) {
    results.push({
      type: 'text',
      content: text.slice(lastIndex),
      href: undefined,
      filePath: undefined,
    });
  }

  return results;
}

/**
 * Convert text containing URLs and file paths into React elements.
 * URLs are rendered as <a> tags that open in new tabs.
 * File paths are rendered as clickable spans that trigger the onFileClick callback.
 */
export function linkifyText(
  text: string,
  onFileClick?: (filePath: string) => void
): React.ReactNode {
  const segments = parseLinks(text);

  if (segments.length === 0) {
    return text;
  }

  // If there's only one text segment with no links, return plain text
  if (segments.length === 1 && segments[0]?.type === 'text') {
    return text;
  }

  return segments.map((segment, index) => {
    if (segment.type === 'link') {
      return (
        <a
          key={index}
          href={segment.href}
          target="_blank"
          rel="noopener noreferrer"
          className="text-link"
        >
          {segment.content}
        </a>
      );
    }
    if (segment.type === 'file' && onFileClick) {
      return (
        <span
          key={index}
          role="button"
          tabIndex={0}
          onClick={() => onFileClick(segment.filePath!)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              onFileClick(segment.filePath!);
            }
          }}
          className="file-path-link"
          title={`Open ${segment.filePath}`}
        >
          {segment.content}
        </span>
      );
    }
    if (segment.type === 'file') {
      // No click handler provided, render as styled but non-interactive
      return (
        <span key={index} className="file-path-text">
          {segment.content}
        </span>
      );
    }
    return <React.Fragment key={index}>{segment.content}</React.Fragment>;
  });
}
