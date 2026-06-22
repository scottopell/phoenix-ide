import { useCallback, useEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api, type LlmLanguageCatalogEntry, type LlmLanguageSetting } from '../api';

const PROMPT_LABELS: Array<[keyof LlmLanguageCatalogEntry['prompts'], string]> = [
  ['base_prompt', 'Base prompt'],
  ['explore_mode_block_template', 'Explore mode block template'],
  ['work_mode_block_template', 'Work mode block template'],
  ['direct_mode_block', 'Direct mode block'],
  ['branch_mode_block_template', 'Branch mode block template'],
  ['sub_agent_suffix', 'Sub-agent suffix'],
  ['next_task_hint_template', 'Next-task hint template'],
  ['pr_autofix_instruction_template', 'PR autofix instruction template'],
];

function PromptBlock({ label, text }: { label: string; text: string }) {
  const [copied, setCopied] = useState(false);
  const timeoutRef = useRef<number | null>(null);

  const copy = useCallback(() => {
    navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      if (timeoutRef.current !== null) window.clearTimeout(timeoutRef.current);
      timeoutRef.current = window.setTimeout(() => setCopied(false), 1200);
    }).catch(() => setCopied(false));
  }, [text]);

  useEffect(() => () => {
    if (timeoutRef.current !== null) window.clearTimeout(timeoutRef.current);
  }, []);

  return (
    <div className="llm-prompt-block">
      <div className="llm-prompt-block__header">
        <h4>{label}</h4>
        <button type="button" className="settings-inline-btn" onClick={copy}>
          {copied ? 'Copied ✓' : 'Copy'}
        </button>
      </div>
      <pre className="llm-prompt-block__pre"><code>{text}</code></pre>
    </div>
  );
}

export function LlmLanguagePage() {
  const navigate = useNavigate();
  const [setting, setSetting] = useState<LlmLanguageSetting | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.getLlmLanguageSetting()
      .then((loaded) => { if (!cancelled) setSetting(loaded); })
      .catch((e) => { if (!cancelled) setError(e instanceof Error ? e.message : String(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, []);

  const select = useCallback((language: string) => {
    if (!setting || setting.language === language) return;
    setSaving(true);
    setError(null);
    api.updateLlmLanguageSetting(language)
      .then((saved) => setSetting(saved))
      .catch((e) => setError(e instanceof Error ? e.message : 'Failed to save LLM language'))
      .finally(() => setSaving(false));
  }, [setting]);

  return (
    <div id="app" className="list-page">
      <main id="main-area">
        <section className="view active">
          <div className="view-header">
            <h2>LLM language prompts</h2>
            <div className="view-header-actions">
              <button type="button" className="settings-inline-btn" onClick={() => navigate(-1)}>
                Back
              </button>
            </div>
          </div>

          <section className="settings-section">
            <h3 className="settings-section__title">Default for new conversations</h3>
            <div className="settings-section__hint">
              Changing this affects new conversations only. Existing conversations stay pinned to the language they were created with.
            </div>
            {setting && (
              <div className="settings-theme-row" role="radiogroup" aria-label="LLM language">
                {setting.languages.map((lang) => {
                  const active = setting.language === lang.id;
                  return (
                    <button
                      key={lang.id}
                      type="button"
                      role="radio"
                      aria-checked={active}
                      className={`settings-theme-btn${active ? ' active' : ''}`}
                      onClick={() => select(lang.id)}
                      disabled={saving}
                      title={lang.description}
                    >
                      {lang.label}
                    </button>
                  );
                })}
              </div>
            )}
            {saving && <div className="settings-section__hint">Saving…</div>}
            {error && <div className="settings-section__error">{error}</div>}
            {!setting && loading && <div className="settings-section__hint">Loading…</div>}
          </section>

          {setting?.languages.map((language) => (
            <section key={language.id} className="settings-section llm-language-card">
              <div className="llm-language-card__title-row">
                <h3 className="settings-section__title">{language.label}</h3>
                {setting.language === language.id && <span className="llm-language-card__current">current default</span>}
              </div>
              <div className="settings-section__hint">{language.description}</div>
              <div className="llm-prompt-grid">
                {PROMPT_LABELS.map(([key, label]) => (
                  <PromptBlock key={key} label={label} text={language.prompts[key]} />
                ))}
              </div>
            </section>
          ))}
        </section>
      </main>
    </div>
  );
}
