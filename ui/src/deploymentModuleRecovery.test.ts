import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, screen } from '@testing-library/react';
import { installDeploymentModuleRecovery } from './deploymentModuleRecovery';

function preloadError(payload: unknown): Event {
  const event = new Event('vite:preloadError', { cancelable: true }) as Event & {
    payload?: unknown;
  };
  event.payload = payload;
  return event;
}

describe('deployment module recovery', () => {
  afterEach(() => {
    document.getElementById('phoenix-module-load-notice')?.remove();
    vi.restoreAllMocks();
  });

  it('preserves the page until the user chooses to reload', () => {
    const reload = vi.fn();
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const uninstall = installDeploymentModuleRecovery({ reload });
    const event = preloadError(new TypeError('Failed to fetch dynamically imported module: /assets/flow.js'));

    window.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(true);
    expect(reload).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toHaveTextContent('unsent attachments will be lost');
    expect(console.error).toHaveBeenCalledWith(
      '[Phoenix] Failed to load a deployed UI module.',
      expect.stringContaining('/assets/flow.js'),
    );

    fireEvent.click(screen.getByRole('button', { name: 'Reload Phoenix' }));
    expect(reload).toHaveBeenCalledOnce();
    uninstall();
  });

  it('consumes persistent failures without duplicating the recovery surface', () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const reload = vi.fn();
    const uninstall = installDeploymentModuleRecovery({ reload });
    const firstEvent = preloadError(new Error('first failure'));
    const secondEvent = preloadError(new Error('persistent failure'));

    window.dispatchEvent(firstEvent);
    window.dispatchEvent(secondEvent);

    expect(firstEvent.defaultPrevented).toBe(true);
    expect(secondEvent.defaultPrevented).toBe(true);
    expect(screen.getAllByRole('alert')).toHaveLength(1);
    expect(reload).not.toHaveBeenCalled();
    uninstall();
  });

  it('allows the warning to be dismissed while preserving the current page', () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const reload = vi.fn();
    const uninstall = installDeploymentModuleRecovery({ reload });
    window.dispatchEvent(preloadError(new Error('optional feature failed')));

    fireEvent.click(screen.getByRole('button', { name: 'Dismiss module load warning' }));

    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(reload).not.toHaveBeenCalled();
    uninstall();
  });
});
