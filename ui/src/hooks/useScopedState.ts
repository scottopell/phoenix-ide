import { useState } from 'react';

/**
 * Synchronously reset state when a logical UI scope changes.
 *
 * Use for conversation-scoped or chain-scoped state that lives in a stable
 * React tree across route-param changes. This intentionally follows React's
 * "adjust state during render" pattern so children never commit with the old
 * scope's value under the new scope key.
 */
export function useScopedState<T>(scopeKey: string | undefined, initialValue: T) {
  const [value, setValue] = useState<T>(initialValue);
  const [trackedScope, setTrackedScope] = useState<string | undefined>(scopeKey);

  if (trackedScope !== scopeKey) {
    setTrackedScope(scopeKey);
    if (!Object.is(value, initialValue)) {
      setValue(initialValue);
    }
    return [initialValue, setValue] as const;
  }

  return [value, setValue] as const;
}
