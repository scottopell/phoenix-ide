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
  ['mermaid_rendering_hint', 'Mermaid rendering hint'],
  ['coordinator_prompt', 'Coordinator prompt'],
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
        <h4 className="llm-prompt-block__title">{label}</h4>
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
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.getLlmLanguageSetting()
      .then((loaded) => { if (!cancelled) setSetting(loaded); })
      .catch((e) => { if (!cancelled) setError(e instanceof Error ? e.message : String(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, []);


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
            <div className="settings-section__hint">
              Choose a language when starting each conversation. Existing conversations keep their selected language.
            </div>
            {error && <div className="settings-section__error">{error}</div>}
            {!setting && loading && <div className="settings-section__hint">Loading…</div>}
          </section>

          {setting?.languages.map((language) => (
            <section key={language.id} className="settings-section llm-language-card">
              <div className="llm-language-card__title-row">
                <h3 className="settings-section__title">{language.label}</h3>
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
