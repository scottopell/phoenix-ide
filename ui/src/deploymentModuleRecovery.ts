import { recordModuleAcquisitionFailure } from './moduleAcquisitionFailure';
import './deploymentModuleRecovery.css';

const NOTICE_ID = 'phoenix-module-load-notice';

type PreloadErrorEvent = Event & { payload?: unknown };

type RecoveryOptions = {
  reload?: () => void;
};

function errorDetail(payload: unknown): string {
  if (payload instanceof Error) return payload.stack || payload.message;
  return String(payload ?? 'Unknown module preload error');
}

function showRecoveryNotice(reload: () => void): void {
  if (document.getElementById(NOTICE_ID)) return;

  const notice = document.createElement('div');
  notice.id = NOTICE_ID;
  notice.className = 'module-load-notice';
  notice.setAttribute('role', 'alert');

  const text = document.createElement('span');
  text.className = 'module-load-notice__text';
  text.textContent = 'Part of the Phoenix UI could not be loaded. Reload when ready; unsent attachments will be lost.';

  const reloadButton = document.createElement('button');
  reloadButton.type = 'button';
  reloadButton.className = 'module-load-notice__reload';
  reloadButton.textContent = 'Reload Phoenix';
  reloadButton.addEventListener('click', reload);

  const dismissButton = document.createElement('button');
  dismissButton.type = 'button';
  dismissButton.className = 'module-load-notice__dismiss';
  dismissButton.setAttribute('aria-label', 'Dismiss module load warning');
  dismissButton.textContent = '×';
  dismissButton.addEventListener('click', () => notice.remove());

  notice.append(text, reloadButton, dismissButton);
  document.body.append(notice);
}

export function installDeploymentModuleRecovery(options: RecoveryOptions = {}): () => void {
  const reload = options.reload ?? (() => window.location.reload());

  const handlePreloadError = (event: Event) => {
    const preloadEvent = event as PreloadErrorEvent;
    recordModuleAcquisitionFailure();
    console.error(
      '[Phoenix] Failed to load a deployed UI module.',
      errorDetail(preloadEvent.payload),
    );

    event.preventDefault();
    showRecoveryNotice(reload);
  };

  window.addEventListener('vite:preloadError', handlePreloadError);
  return () => {
    window.removeEventListener('vite:preloadError', handlePreloadError);
    document.getElementById(NOTICE_ID)?.remove();
  };
}
