# Phoenix IDE Frontend

React + TypeScript + Vite.

## Patterns

### Effect-driven reconnection (connectEpoch)

When a component needs to reconnect to a server resource (SSE, WebSocket) based on
user action or automatic retry, use a state counter as the effect dependency instead
of calling a loose function from the effect body.

```tsx
const [connectEpoch, setConnectEpoch] = useState(0);

useEffect(() => {
  const es = new EventSource('/api/some-stream');
  // ... setup handlers ...

  es.addEventListener('error', () => {
    // Auto-retry: bump epoch to trigger effect re-run
    setTimeout(() => setConnectEpoch(e => e + 1), 2000);
  });

  return () => es.close();
}, [active, connectEpoch]);  // effect re-runs when epoch bumps

// Manual retry button: same mechanism
<button onClick={() => setConnectEpoch(e => e + 1)}>Retry</button>
```

Why this works: the effect owns the connection lifecycle. Reconnection is a data flow
concern (epoch changes -> effect re-runs -> new connection) rather than an imperative
function call. No loose `connect()` function, no lint suppression needed, cleanup
handles the old connection automatically.

See `CredentialHelperPanel.tsx` for the canonical implementation.
