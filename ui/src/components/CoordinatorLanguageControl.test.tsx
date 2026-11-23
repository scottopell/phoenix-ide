import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { api, type LlmLanguageSetting } from '../api';
import { CoordinatorLanguageControl } from './CoordinatorLanguageControl';

vi.mock('../api', () => ({ api: { getCoordinatorLanguage: vi.fn(), updateCoordinatorLanguage: vi.fn() } }));

const view = (language: string): LlmLanguageSetting => ({
  language,
  available: ['phoenix-native', 'caveman', 'adhd'],
  languages: ['phoenix-native', 'caveman', 'adhd'].map(id => ({ id, label: id === 'adhd' ? 'ADHD' : id, description: `${id} description`, prompts: {} as LlmLanguageSetting['languages'][number]['prompts'] })),
});

describe('Ember language (REQ-LL-003)', () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.getCoordinatorLanguage).mockResolvedValue(view('caveman'));
  });

  it('loads Ember selection, waits for save and retains it across remount', async () => {
    const first = render(<CoordinatorLanguageControl />);
    const picker = await screen.findByRole('combobox', { name: 'Ember language' });
    await waitFor(() => expect(picker).toHaveValue('caveman'));
    let resolve!: (value: LlmLanguageSetting) => void;
    vi.mocked(api.updateCoordinatorLanguage).mockImplementation(() => new Promise(done => { resolve = done; }));
    fireEvent.change(picker, { target: { value: 'adhd' } });
    expect(api.updateCoordinatorLanguage).toHaveBeenCalledWith('adhd');
    expect(picker).toBeDisabled();
    expect(picker).toHaveValue('caveman');
    await act(async () => resolve(view('adhd')));
    expect(picker).toHaveValue('adhd');
    first.unmount();
    vi.mocked(api.getCoordinatorLanguage).mockResolvedValue(view('adhd'));
    render(<CoordinatorLanguageControl />);
    await waitFor(() => expect(screen.getByRole('combobox')).toHaveValue('adhd'));
  });

  it('keeps the saved selection on refusal and lets the user retry', async () => {
    vi.mocked(api.updateCoordinatorLanguage).mockRejectedValueOnce(new Error('Ember must be idle')).mockResolvedValue(view('adhd'));
    render(<CoordinatorLanguageControl />);
    const picker = screen.getByRole('combobox');
    await waitFor(() => expect(picker).toBeEnabled());
    fireEvent.change(picker, { target: { value: 'adhd' } });
    expect(await screen.findByText('Ember must be idle')).toBeVisible();
    expect(picker).toHaveValue('caveman');
    fireEvent.change(picker, { target: { value: 'adhd' } });
    await waitFor(() => expect(picker).toHaveValue('adhd'));
  });

  it('reports a load failure with a retry', async () => {
    vi.mocked(api.getCoordinatorLanguage).mockRejectedValueOnce(new Error('Offline'));
    render(<CoordinatorLanguageControl />);
    fireEvent.click(await screen.findByRole('button', { name: 'Retry language loading' }));
    await waitFor(() => expect(screen.getByRole('combobox')).toHaveValue('caveman'));
  });
});
