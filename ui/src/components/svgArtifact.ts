import type { Message, ToolResultContent } from '../api';

export interface SvgArtifact {
  artifact_id: string;
  conversation_id: string;
  title: string;
  description: string;
  width: number;
  height: number;
  validation: 'accepted_static_svg';
}

export function svgArtifactFromResult(name: string, result: Message | undefined): SvgArtifact | null {
  if (name !== 'present_svg' || !result) return null;
  const content = result.content as ToolResultContent;
  if (content.is_error || content.error) return null;
  const text = content.content || content.result;
  if (typeof text !== 'string' || text.length > 16384) return null;
  try {
    const value: unknown = JSON.parse(text);
    if (!value || typeof value !== 'object') return null;
    const v = value as Record<string, unknown>;
    const boundedText = (x: unknown, limit: number): x is string => typeof x === 'string' && x.trim().length > 0 && Array.from(x).length <= limit;
    const identifier = (x: unknown): x is string => typeof x === 'string' && /^[a-zA-Z0-9_-]{1,128}$/.test(x);
    const dimension = (x: unknown): x is number => typeof x === 'number' && Number.isFinite(x) && x > 0 && x <= 16384;
    if (!identifier(v['artifact_id']) || !identifier(v['conversation_id']) || v['conversation_id'] !== result.conversation_id
      || !boundedText(v['title'], 200) || !boundedText(v['description'], 2000)
      || !dimension(v['width']) || !dimension(v['height']) || v['validation'] !== 'accepted_static_svg') return null;
    return { artifact_id: v['artifact_id'], conversation_id: v['conversation_id'], title: v['title'], description: v['description'], width: v['width'], height: v['height'], validation: v['validation'] };
  } catch {
    return null;
  }
}
