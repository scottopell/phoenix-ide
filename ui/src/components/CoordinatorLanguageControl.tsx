import { useEffect, useState } from 'react';
import { api, type LlmLanguageSetting } from '../api';

// REQ-LL-003
export function CoordinatorLanguageControl() {
  const [view, setView] = useState<LlmLanguageSetting | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setFeedback(null);
    void api.getCoordinatorLanguage().then(next => {
      if (!cancelled) setView(next);
    }).catch((error: unknown) => {
      if (!cancelled) setFeedback(error instanceof Error ? error.message : 'Failed to load Ember language');
    }).finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [retry]);

  const save = async (language: string) => {
    setSaving(true);
    setFeedback(null);
    try {
      setView(await api.updateCoordinatorLanguage(language));
      setFeedback('Saved. Ember will use this language for its next response.');
    } catch (error) {
      setFeedback(error instanceof Error ? error.message : 'Failed to save Ember language');
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="coordinator-language-control" aria-label="Ember language">
      <label>
        Ember language{' '}
        <select aria-label="Ember language" value={view?.language ?? ''} disabled={loading || saving || !view} onChange={event => void save(event.target.value)}>
          {!view && <option value="">{loading ? 'Loading…' : 'Unavailable'}</option>}
          {view?.languages.map(language => <option key={language.id} value={language.id}>{language.label}</option>)}
        </select>
      </label>
      <p>{view?.languages.find(language => language.id === view.language)?.description}</p>
      <p>Applies only to Ember. Change while Ember is idle.</p>
      {saving && <p role="status">Saving…</p>}
      {feedback && <p role="status">{feedback}</p>}
      {!loading && !view && <button type="button" onClick={() => setRetry(value => value + 1)}>Retry language loading</button>}
    </section>
  );
}
