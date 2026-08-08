import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { installDeploymentModuleRecovery } from './deploymentModuleRecovery';

function preloadError(payload: unknown): Event {
  const event = new Event('vite:preloadError', { cancelable: true }) as Event & {
    payload?: unknown;
  };
  event.payload = payload;
  return event;
}

describe('deployment module recovery', () => {
  beforeEach(() => {
    sessionStorage.clear();
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('reloads once when a deployed module cannot be loaded', () => {
    const reload = vi.fn();
    const uninstall = installDeploymentModuleRecovery({ now: () => 1_000, reload });
    const event = preloadError(new TypeError('Failed to fetch dynamically imported module: /assets/flow.js'));

    window.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(true);
    expect(reload).toHaveBeenCalledOnce();
    expect(console.error).toHaveBeenCalledWith(
      '[Phoenix] Failed to load a deployed UI module.',
      expect.stringContaining('/assets/flow.js'),
    );
    uninstall();
  });

  it('does not reload-loop when the coherent generation is still unavailable', () => {
    const firstReload = vi.fn();
    const uninstallFirst = installDeploymentModuleRecovery({ now: () => 1_000, reload: firstReload });
    window.dispatchEvent(preloadError(new Error('first failure')));
    uninstallFirst();

    const secondReload = vi.fn();
    const uninstallSecond = installDeploymentModuleRecovery({ now: () => 2_000, reload: secondReload });
    const secondEvent = preloadError(new Error('persistent failure'));
    window.dispatchEvent(secondEvent);

    expect(firstReload).toHaveBeenCalledOnce();
    expect(secondReload).not.toHaveBeenCalled();
    expect(secondEvent.defaultPrevented).toBe(false);
    uninstallSecond();
  });

  it('uses one guard across routes in the same deployment session', () => {
    const reload = vi.fn();
    const uninstall = installDeploymentModuleRecovery({ now: () => 1_000, reload });
    window.dispatchEvent(preloadError(new Error('first route failure')));
    history.pushState({}, '', '/another-route');
    window.dispatchEvent(preloadError(new Error('second route failure')));

    expect(reload).toHaveBeenCalledOnce();
    uninstall();
  });

  it('allows another recovery after the bounded window', () => {
    const firstReload = vi.fn();
    const uninstallFirst = installDeploymentModuleRecovery({ now: () => 1_000, reload: firstReload });
    window.dispatchEvent(preloadError(new Error('first failure')));
    uninstallFirst();

    const laterReload = vi.fn();
    const uninstallLater = installDeploymentModuleRecovery({ now: () => 31_001, reload: laterReload });
    window.dispatchEvent(preloadError(new Error('later failure')));

    expect(laterReload).toHaveBeenCalledOnce();
    uninstallLater();
  });
});
