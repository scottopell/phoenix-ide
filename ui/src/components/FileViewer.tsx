import { useEffect, useMemo, useState } from 'react';
import { AlertCircle, Loader2 } from 'lucide-react';
import { ViewerShell } from './viewer/ViewerShell';
import { ProseReader } from './ProseReader';
import type { ProseReaderProps } from './ProseReader';

type ReadFileResult =
  | { kind: 'text'; content: string; encoding: string; file_type: string }
  | { kind: 'image'; mime_type: string; url: string; file_type: string };

type FileViewerProps = Omit<ProseReaderProps, 'content'>;

async function readFile(path: string): Promise<ReadFileResult> {
  const response = await fetch(`/api/files/read?path=${encodeURIComponent(path)}`);
  if (!response.ok) {
    const error = await response.json().catch(() => ({ error: 'Unknown error' }));
    throw new Error(error.error || 'Failed to read file');
  }
  const data = await response.json();
  if (data.kind === 'image') {
    return {
      kind: 'image',
      mime_type: data.mime_type,
      url: data.url,
      file_type: data.file_type,
    };
  }
  return {
    kind: 'text',
    content: data.content,
    encoding: data.encoding ?? 'utf-8',
    file_type: data.file_type ?? 'text',
  };
}

function ImagePreview({ fileName, url }: { fileName: string; url: string }) {
  return (
    <div className="image-preview">
      <img src={url} alt={fileName} className="image-preview-img" />
    </div>
  );
}

export function FileViewer({
  filePath,
  rootDir,
  onClose,
  onSendNotes,
  patchContext,
  inline,
}: FileViewerProps) {
  const [fileData, setFileData] = useState<ReadFileResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const absolutePath = useMemo(() => {
    if (filePath.startsWith('/')) return filePath;
    return rootDir.endsWith('/') ? rootDir + filePath : rootDir + '/' + filePath;
  }, [filePath, rootDir]);
  const fileName = filePath.split('/').pop() || filePath;

  useEffect(() => {
    let cancelled = false;
    async function load() {
      setLoading(true);
      setError(null);
      setFileData(null);
      try {
        const result = await readFile(absolutePath);
        if (!cancelled) setFileData(result);
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : 'Failed to load file');
      } finally {
        if (!cancelled) setLoading(false);
      }
    }
    load();
    return () => { cancelled = true; };
  }, [absolutePath]);

  if (fileData?.kind === 'text') {
    const proseProps: ProseReaderProps = {
      filePath,
      rootDir,
      content: fileData.content,
      onClose,
      onSendNotes,
    };
    if (patchContext) proseProps.patchContext = patchContext;
    if (inline !== undefined) proseProps.inline = inline;
    return <ProseReader {...proseProps} />;
  }

  return (
    <ViewerShell
      mode={inline ? 'inline' : 'overlay'}
      ariaLabel={`File viewer: ${fileName}`}
      title={fileName}
      titleTooltip={absolutePath}
      noteCount={0}
      onToggleNotes={() => undefined}
      onSend={() => undefined}
      onClose={onClose}
    >
      <div className="prose-reader-content">
        {loading ? (
          <div className="prose-reader-loading">
            <Loader2 size={32} className="spinning" />
            <span>Loading file...</span>
          </div>
        ) : error ? (
          <div className="prose-reader-error">
            <AlertCircle size={32} />
            <span>{error}</span>
            <button onClick={onClose}>Close</button>
          </div>
        ) : fileData?.kind === 'image' ? (
          <ImagePreview fileName={fileName} url={fileData.url} />
        ) : null}
      </div>
    </ViewerShell>
  );
}
