import React from 'react';
import { resolveFilePathCopyValues } from './filePathCopy';
import type { FilePathCopyContext } from './linkify';

export interface FilePathLinkProps {
  filePath: string;
  onFileClick?: ((filePath: string) => void) | undefined;
  filePathCopyContext?: FilePathCopyContext | undefined;
  children?: React.ReactNode;
}

export function FilePathLink({ filePath, onFileClick, filePathCopyContext, children }: FilePathLinkProps) {
  if (!onFileClick) {
    return <span className="file-path-text">{children ?? filePath}</span>;
  }
  const copyValues = filePathCopyContext
    ? resolveFilePathCopyValues(filePath, filePathCopyContext.rootDir)
    : undefined;
  return (
    <span
      role="button"
      tabIndex={0}
      onClick={() => onFileClick(filePath)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onFileClick(filePath);
        }
      }}
      className="file-path-link"
      title={`Open ${filePath}`}
      data-file-path={filePath}
      {...(copyValues
        ? {
            'data-file-absolute-path': copyValues.absolutePath,
            'data-file-relative-path': copyValues.relativePath,
          }
        : {})}
    >
      {children ?? filePath}
    </span>
  );
}
