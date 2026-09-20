import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { SvgArtifactCard } from './SvgArtifactCard';
import { svgArtifactFromResult, type SvgArtifact } from './svgArtifact';
import type { Message } from '../api';
import { SvgArtifactAccessContext } from '../contexts/SvgArtifactAccessContext';

const artifact: SvgArtifact = { artifact_id: 'artifact-1', conversation_id: 'conv-1', title: 'Disk usage', description: 'Directory sizes in GiB.', width: 800, height: 400, validation: 'accepted_static_svg' };
const result = (value: unknown): Message => ({ message_id: 'm', conversation_id: 'conv-1', sequence_id: 2, message_type: 'tool', content: { tool_use_id: 'tool-1', content: JSON.stringify(value), is_error: false }, display_data: null, created_at: '' });
afterEach(() => vi.unstubAllGlobals());

describe('SVG artifact presentation', () => {
  it('accepts only bounded successful metadata for this conversation', () => {
    expect(svgArtifactFromResult('present_svg', result(artifact))).toEqual(artifact);
    for (const invalid of [{ ...artifact, width: 0 }, { ...artifact, height: 20000 }, { ...artifact, artifact_id: '../x' }, { ...artifact, conversation_id: 'other' }, { ...artifact, title: '' }, { ...artifact, validation: 'unsafe' }]) {
      expect(svgArtifactFromResult('present_svg', result(invalid))).toBeNull();
    }
    expect(svgArtifactFromResult('present_svg', result({ ...artifact, title: '😀'.repeat(200) }))).not.toBeNull();
    expect(svgArtifactFromResult('present_svg', result({ ...artifact, title: '😀'.repeat(201) }))).toBeNull();
    expect(svgArtifactFromResult('present_svg', result({ ...artifact, description: 'x'.repeat(2001) }))).toBeNull();
    expect(svgArtifactFromResult('bash', result(artifact))).toBeNull();
    const failed = result(artifact);
    failed.content = { tool_use_id: 'tool-1', content: JSON.stringify(artifact), is_error: true };
    expect(svgArtifactFromResult('present_svg', failed)).toBeNull();
  });

  it.each([
    '67e55044-10b1-426f-9247-bb680e5fe0c8',
    '67E55044-10B1-426F-9247-BB680E5FE0C8',
    '67e5504410b1426f9247bb680e5fe0c8',
    '67E5504410B1426F9247BB680E5FE0C8',
    '{67e55044-10b1-426f-9247-bb680e5fe0c8}',
    '{67E55044-10B1-426F-9247-BB680E5FE0C8}',
    'urn:uuid:67e55044-10b1-426f-9247-bb680e5fe0c8',
    'urn:uuid:67E55044-10B1-426F-9247-BB680E5FE0C8',
    '{00000000-0000-0000-0000-000000000000}',
    'urn:uuid:ffffffff-ffff-ffff-ffff-ffffffffffff',
  ])('retains supported conversation UUID form %s and encodes its artifact URLs', (conversationId) => {
    const value = { ...artifact, conversation_id: conversationId };
    const message = { ...result(value), conversation_id: conversationId };
    const parsed = svgArtifactFromResult('present_svg', message);
    expect(parsed).toEqual(value);
    if (!parsed) throw new Error('expected accepted artifact');
    render(<SvgArtifactCard artifact={parsed} />);
    const url = `/api/conversations/${encodeURIComponent(conversationId)}/svg-artifacts/artifact-1`;
    expect(screen.getByRole('img')).toHaveAttribute('src', url);
    expect(screen.getByRole('link', { name: 'Download SVG' })).toHaveAttribute('href', `${url}/download`);
  });

  it.each([
    '../owner', 'owner/other', 'owner%2fother', 'owner\n',
    '{67e5504410b1426f9247bb680e5fe0c8}',
    'urn:uuid:67e5504410b1426f9247bb680e5fe0c8',
    'URN:UUID:67e55044-10b1-426f-9247-bb680e5fe0c8',
    '{67e55044-10b1-426f-9247-bb680e5fe0c8',
    'urn:uuid:67e55044-10b1-426f-9247-bb680e5fe0cg',
    'urn:uuid:67e55044-10b1-426f-9247-bb680e5fe0c8/../other',
  ])('rejects malformed wrapped UUIDs and unsafe conversation paths: %s', (conversationId) => {
    const value = { ...artifact, conversation_id: conversationId };
    expect(svgArtifactFromResult('present_svg', { ...result(value), conversation_id: conversationId })).toBeNull();
  });

  it('uses only the share token and artifact ID for all shared representations', async () => {
    const fetchSource = vi.fn().mockResolvedValue({ ok: true, text: async () => '<svg/>' });
    vi.stubGlobal('fetch', fetchSource);
    render(<SvgArtifactAccessContext.Provider value={{ kind: 'share', token: 'token/with space' }}><SvgArtifactCard artifact={artifact} /></SvgArtifactAccessContext.Provider>);
    const url = '/api/share/token%2Fwith%20space/svg-artifacts/artifact-1';
    expect(screen.getByRole('img')).toHaveAttribute('src', url);
    expect(screen.getByRole('link', { name: 'Download SVG' })).toHaveAttribute('href', `${url}/download`);
    fireEvent.click(screen.getByRole('button', { name: 'Expand visualization' }));
    expect(screen.getAllByRole('img').every((img) => img.getAttribute('src') === url)).toBe(true);
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    fireEvent.click(screen.getByRole('button', { name: 'View source' }));
    await screen.findByText('<svg/>');
    expect(fetchSource).toHaveBeenCalledWith(`${url}/source`, expect.objectContaining({ credentials: 'same-origin' }));
  });

  it('shows dimensions, loading, failure fallback and ownership-scoped download', () => {
    render(<SvgArtifactCard artifact={artifact} />);
    const image = screen.getByRole('img');
    expect(image).toHaveAttribute('width', '800');
    expect(image).toHaveAttribute('height', '400');
    expect(image).toHaveAttribute('src', '/api/conversations/conv-1/svg-artifacts/artifact-1');
    expect(screen.getByRole('status')).toHaveTextContent('Loading');
    fireEvent.load(image);
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    fireEvent.error(image);
    expect(screen.getByRole('alert')).toHaveTextContent('Visualization unavailable or unreadable. Directory sizes in GiB.');
    expect(screen.getByRole('link', { name: 'Download SVG' })).toHaveAttribute('href', '/api/conversations/conv-1/svg-artifacts/artifact-1/download');
  });

  it('expands in an accessible dialog, zooms, dismisses with Escape and restores focus', async () => {
    render(<SvgArtifactCard artifact={artifact} />);
    const trigger = screen.getByRole('button', { name: 'Expand visualization' });
    trigger.focus();
    fireEvent.click(trigger);
    const dialog = screen.getByRole('dialog', { name: artifact.title });
    fireEvent.click(screen.getByRole('button', { name: 'Zoom in' }));
    expect(screen.getByLabelText('Zoom level')).toHaveTextContent('150%');
    fireEvent.keyDown(dialog, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it('shows source as inert text and reports source read failures', async () => {
    const markup = '<svg><script>alert(1)</script></svg>';
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, text: async () => markup }));
    render(<SvgArtifactCard artifact={artifact} />);
    fireEvent.click(screen.getByRole('button', { name: 'View source' }));
    expect(await screen.findByText(markup)).toBeInTheDocument();
    expect(document.querySelector('.svg-artifact-source svg')).toBeNull();
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false }));
    fireEvent.click(screen.getByRole('button', { name: 'View source' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Source unavailable');
  });
});
