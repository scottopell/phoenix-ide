/**
 * PatchFileSummary Component
 * 
 * Implements REQ-PF-014: Parse unified diffs and display clickable file list
 * with change counts at the end of patch output.
 */

import { useMemo } from 'react';
import { FileCode, ChevronRight } from 'lucide-react';

interface FileChanges {
  filePath: string;
  modifiedLines: Set<number>;
  firstModifiedLine: number;
}

interface PatchFileSummaryProps {
  patchOutput: string;
  onFileClick: (filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void;
}

/**
 * Extract all unique files and their changes from patch output.
 * Parses unified diff format to find modified line numbers.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function extractFileChanges(patchOutput: string): FileChanges[] {
  const fileChangesMap = new Map<string, Set<number>>();
  const lines = patchOutput.split('\n');
  let currentFile: string | null = null;
  let currentLine = 0;

  for (const line of lines) {
    // Match file header: +++ b/path/to/file.ext or +++ path/to/file.ext
    const fileMatch = line.match(/^\+{3}\s+(?:b\/)?(.+)$/);
    if (fileMatch) {
      currentFile = fileMatch[1];
      if (!fileChangesMap.has(currentFile)) {
        fileChangesMap.set(currentFile, new Set());
      }
      continue;
    }

    // Parse hunk headers: @@ -start,count +start,count @@
    const hunkHeader = line.match(/@@ -\d+,?\d* \+(\d+),?\d* @@/);
    if (hunkHeader && currentFile) {
      currentLine = parseInt(hunkHeader[1], 10) - 1;
      continue;
    }

    // Track added/modified lines (lines starting with +)
    if (currentFile && line.startsWith('+') && !line.startsWith('+++')) {
      fileChangesMap.get(currentFile)!.add(currentLine + 1);
    }

    // Increment line counter for context and additions (not deletions)
    if (currentFile && !line.startsWith('-')) {
      currentLine++;
    }
  }

  // Convert map to array with first modified line info
  const result: FileChanges[] = [];
  for (const [filePath, modifiedLines] of fileChangesMap) {
    if (modifiedLines.size > 0) {
      const firstModifiedLine = Math.min(...Array.from(modifiedLines));
      result.push({ filePath, modifiedLines, firstModifiedLine });
    }
  }

  // Sort by file path for consistent display
  result.sort((a, b) => a.filePath.localeCompare(b.filePath));

  return result;
}

/**
 * Detect if text contains unified diff content.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function containsUnifiedDiff(text: string): boolean {
  return text.includes('@@') && (text.includes('+++') || text.includes('---'));
}

export function PatchFileSummary({ patchOutput, onFileClick }: PatchFileSummaryProps) {
  const fileChanges = useMemo(() => extractFileChanges(patchOutput), [patchOutput]);

  if (fileChanges.length === 0) {
    return null;
  }

  return (
    <div className="patch-file-summary">
      <div className="patch-file-summary-header">
        Modified files:
      </div>
      <div className="patch-file-summary-list">
        {fileChanges.map(({ filePath, modifiedLines, firstModifiedLine }) => (
          <button
            key={filePath}
            className="patch-file-link"
            onClick={() => onFileClick(filePath, modifiedLines, firstModifiedLine)}
          >
            <FileCode size={16} className="patch-file-icon" />
            <span className="patch-file-name">{filePath}</span>
            <span className="patch-file-changes">
              ({modifiedLines.size} change{modifiedLines.size !== 1 ? 's' : ''})
            </span>
            <ChevronRight size={16} className="patch-file-chevron" />
          </button>
        ))}
      </div>
    </div>
  );
}
