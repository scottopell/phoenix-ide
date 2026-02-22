/**
 * RecentFilesStrip — collapsed panel shows last 5 opened files as icons
 * REQ-FE-005, REQ-FE-006
 */

import { FileText, FileCode, Settings, File } from 'lucide-react';
import type { RecentFile } from '../../hooks/useRecentFiles';

interface Props {
  files: RecentFile[];
  onFileClick: (path: string) => void;
}

function iconForName(name: string) {
  const ext = name.split('.').pop()?.toLowerCase();
  if (!ext) return <File size={18} />;
  if (['md', 'markdown', 'txt'].includes(ext)) return <FileText size={18} />;
  if (['rs', 'ts', 'tsx', 'js', 'jsx', 'py', 'go', 'java', 'cpp', 'c', 'h', 'css', 'html', 'sh', 'json', 'yaml', 'toml', 'xml', 'sql'].includes(ext)) return <FileCode size={18} />;
  if (['cfg', 'ini', 'env', 'conf'].includes(ext)) return <Settings size={18} />;
  return <File size={18} />;
}

export function RecentFilesStrip({ files, onFileClick }: Props) {
  if (files.length === 0) return null;

  return (
    <div className="fe-recent-strip">
      {files.map(f => (
        <button
          key={f.path}
          className="fe-recent-icon"
          onClick={() => onFileClick(f.path)}
          title={f.name}
        >
          {iconForName(f.name)}
        </button>
      ))}
    </div>
  );
}
