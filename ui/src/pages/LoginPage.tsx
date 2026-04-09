import { useState, useCallback, type FormEvent } from 'react';
import { api } from '../api';

interface LoginPageProps {
  onSuccess: () => void;
}

export function LoginPage({ onSuccess }: LoginPageProps) {
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);

  const handleSubmit = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      setError('');
      setLoading(true);
      try {
        await api.login(password);
        onSuccess();
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Login failed');
      } finally {
        setLoading(false);
      }
    },
    [password, onSuccess],
  );

  return (
    <div className="login-page">
      <div className="login-card">
        <div className="login-header">
          <img src="/phoenix.svg" alt="Phoenix" className="login-logo" />
          <h1 className="login-title">Phoenix IDE</h1>
        </div>
        <form onSubmit={handleSubmit} className="login-form">
          <input
            type="password"
            className="login-input"
            placeholder="Password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoFocus
            disabled={loading}
          />
          <button
            type="submit"
            className="login-button"
            disabled={loading || password.length === 0}
          >
            {loading ? 'Signing in...' : 'Sign in'}
          </button>
          {error && <div className="login-error">{error}</div>}
        </form>
      </div>
    </div>
  );
}
