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

// Regex for matching file paths that look like real files
// Matches:
//   - Absolute paths: /home/user/file.md
//   - Relative paths: ./file.md, ../dir/file.md  
//   - Project paths: src/api/mod.rs (must contain / and have extension)
// Must have a recognized extension to avoid false positives
// Note: Uses lookbehind for proper word boundary detection
// eslint-disable-next-line no-useless-escape
const FILE_PATH_REGEX = /(?:^|(?<=[\s`"'(\[]))(?:(?:\/[\w.-]+)+|(?:\.\.?\/[\w./-]+)|(?:[\w.-]+\/[\w./-]+))\.(?:md|markdown|rs|ts|tsx|js|jsx|py|go|json|yaml|yml|toml|txt|css|scss|html|htm|vue|svelte|sh|bash|sql|graphql|proto|xml|ini|env|conf|cfg|lock|c|h|cpp|hpp|java|kt|swift|rb|php|ex|exs|hs|ml|zig|scala)(?=[\s`"')\],:;!?]|$)/g;

export interface LinkifySegment {
  type: 'text' | 'link' | 'file';
  content: string;
  href?: string;
  filePath?: string;
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
      });
    }

    // Add the match
    if (m.type === 'link') {
      results.push({
        type: 'link',
        content: m.content,
        href: m.href,
      });
    } else {
      results.push({
        type: 'file',
        content: m.content,
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
  if (segments.length === 1 && segments[0].type === 'text') {
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
