import { CodexLoginPanel } from '../components/CodexLoginPanel';

export function CodexLoginPage() {
  return (
    <div className="login-page">
      <div className="login-card codex-login-card">
        <div className="login-header">
          <img src="/phoenix.svg" alt="Phoenix" className="login-logo" />
          <h1 className="login-title">Sign in with Codex</h1>
          <p className="codex-login-subtitle">
            Use your ChatGPT Plus, Pro, Team, or Enterprise subscription via OpenAI Codex.
          </p>
        </div>
        <CodexLoginPanel />
      </div>
    </div>
  );
}
