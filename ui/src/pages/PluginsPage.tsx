import { useCallback, useEffect, useState } from 'react';
import { api, type PluginStatus, type PluginComponentStatus } from '../api';
import './PluginsPage.css';

function ComponentFailures({ label, outcomes }: { label: string; outcomes: PluginComponentStatus[] | undefined }) {
  return outcomes?.filter((item) => item.status !== 'loaded').map((item) => (
    <p key={item.name} className="plugins-page__mcp-disabled">
      {label} {item.name}: {item.status} — {'reason' in item ? item.reason : ''}
    </p>
  ));
}
export function PluginsPage() {
  const [plugins, setPlugins] = useState<PluginStatus[]>([]);
  const [root, setRoot] = useState('');
  const [loading, setLoading] = useState(true);
  const [source, setSource] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [confirmingRemove, setConfirmingRemove] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const showToast = setNotice;
  const showError = setNotice;

  const refresh = useCallback(async () => {
    try {
      const response = await api.getPlugins();
      setPlugins(response.plugins);
      setRoot(response.root);
    } catch (e) {
      showError(`Failed to load plugins: ${(e as Error).message}`);
    } finally {
      setLoading(false);
    }
  }, [showError]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const add = useCallback(async () => {
    const trimmed = source.trim();
    if (!trimmed) return;
    setBusy('add');
    try {
      const result = await api.addPlugin(trimmed);
      showToast(`Added plugin ${result.name}`);
      setSource('');
      await refresh();
    } catch (e) {
      showError((e as Error).message);
    } finally {
      setBusy(null);
    }
  }, [source, showToast, showError, refresh]);

  const toggle = useCallback(
    async (plugin: PluginStatus) => {
      setBusy(plugin.name);
      try {
        await api.setPluginEnabled(plugin.name, !plugin.enabled);
        showToast(`${plugin.enabled ? 'Disabled' : 'Enabled'} ${plugin.name}`);
        await refresh();
      } catch (e) {
        showError((e as Error).message);
      } finally {
        setBusy(null);
      }
    },
    [showToast, showError, refresh],
  );

  const remove = useCallback(
    async (plugin: PluginStatus) => {
      setBusy(plugin.name);
      try {
        await api.removePlugin(plugin.name);
        showToast(`Removed ${plugin.name} (data retained)`);
        await refresh();
      } catch (e) {
        showError((e as Error).message);
      } finally {
        setConfirmingRemove(null);
        setBusy(null);
      }
    },
    [showToast, showError, refresh],
  );

  const purge = useCallback(
    async (plugin: PluginStatus) => {
      setBusy(plugin.name);
      try {
        await api.purgePluginData(plugin.name);
        showToast(`Purged data for ${plugin.name}`);
        await refresh();
      } catch (e) {
        showError((e as Error).message);
      } finally {
        setBusy(null);
      }
    },
    [showToast, showError, refresh],
  );

  return (
    <div className="plugins-page">
      {notice && <p className="plugins-page__notice" role="status">{notice}</p>}
      <header className="plugins-page__header">
        <h2>Plugins</h2>
        <p className="plugins-page__root">
          root <code>{root || '…'}</code>
        </p>
      </header>

      <div className="plugins-page__add">
        <input
          type="text"
          placeholder="local plugin directory or git URL"
          value={source}
          onChange={(e) => setSource(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void add();
          }}
        />
        <button type="button" onClick={() => void add()} disabled={busy === 'add' || !source.trim()}>
          {busy === 'add' ? 'Adding…' : 'Add'}
        </button>
      </div>

      {loading ? (
        <p className="plugins-page__empty">…</p>
      ) : plugins.length === 0 ? (
        <p className="plugins-page__empty">No plugins installed.</p>
      ) : (
        <ul className="plugins-page__list">
          {plugins.map((plugin) => (
            <li key={plugin.name} className="plugins-page__row">
              <div className="plugins-page__row-main">
                <span className="plugins-page__status">
                  {plugin.missing || plugin.load_error ? '✗' : plugin.enabled ? '✓' : '⏸'}
                </span>
                <span className="plugins-page__name">{plugin.name}</span>
                {plugin.version ? <span className="plugins-page__version">{plugin.version}</span> : null}
                <span className={`plugins-page__provenance plugins-page__provenance--${plugin.provenance}`}>
                  {plugin.provenance}
                </span>
                {!plugin.missing && (
                  <span className="plugins-page__counts">
                    {plugin.skills?.filter((s) => s.status === 'loaded').length ?? 0} skills · {plugin.mcp_servers?.filter((s) => s.status === 'loaded').length ?? 0} servers
                  </span>
                )}
              </div>
              {plugin.description ? (
                <p className="plugins-page__description">{plugin.description}</p>
              ) : null}
              {plugin.load_error ? <p className="plugins-page__mcp-disabled">Load failed: {plugin.load_error}</p> : null}
              <ComponentFailures label="Skill" outcomes={plugin.skills} />
              <ComponentFailures label="MCP" outcomes={plugin.mcp_servers} />
              {plugin.skills_disabled_reason ? (
                <p className="plugins-page__mcp-disabled">Skills disabled: {plugin.skills_disabled_reason}</p>
              ) : null}
              {plugin.mcp_disabled_reason ? (
                <p className="plugins-page__mcp-disabled">MCP disabled: {plugin.mcp_disabled_reason}</p>
              ) : null}
              <div className="plugins-page__actions">
                {plugin.missing ? (
                  <button
                    type="button"
                    onClick={() => void purge(plugin)}
                    disabled={busy === plugin.name}
                  >
                    Purge data
                  </button>
                ) : (
                  <>
                    <button
                      type="button"
                      onClick={() => void toggle(plugin)}
                      disabled={busy === plugin.name}
                    >
                      {plugin.enabled ? 'Disable' : 'Enable'}
                    </button>
                    {confirmingRemove === plugin.name ? (
                      <>
                        <span className="plugins-page__confirm-label">remove? data retained</span>
                        <button type="button" className="danger" onClick={() => void remove(plugin)} disabled={busy === plugin.name}>
                          Confirm
                        </button>
                        <button type="button" onClick={() => setConfirmingRemove(null)}>
                          Cancel
                        </button>
                      </>
                    ) : (
                      <button
                        type="button"
                        className="danger"
                        onClick={() => setConfirmingRemove(plugin.name)}
                        disabled={busy === plugin.name}
                      >
                        Remove
                      </button>
                    )}
                  </>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
