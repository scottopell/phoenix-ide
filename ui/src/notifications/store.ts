// Browser/API adapter for the notification policy.
//
// Owns everything the pure reducer in `./policy` deliberately cannot: the
// live `Notification` registry, the settings API round-trips, and reading
// ambient browser facts (permission, focus, active route) into a
// `NotificationEnv`. The reducer decides; this runs the decision.

import { api, type NotificationSettings } from '../api';
import {
  type BrowserPermission,
  type NotificationEffect,
  type NotificationEnv,
  type NotificationPolicyEvent,
  type NotificationPolicyState,
  initialPolicyState,
  notificationPolicyReducer,
} from './policy';

const coordinatorConversationIds = new Set<string>();

export function registerCoordinatorNotificationTarget(conversationId: string): void {
  coordinatorConversationIds.add(conversationId);
}

export function notificationRoute(conversation: { id: string; slug?: string | null }): string {
  return coordinatorConversationIds.has(conversation.id)
    ? `/global/${conversation.id}`
    : `/c/${conversation.slug ?? conversation.id}`;
}

export function getBrowserNotificationPermission(): BrowserPermission {
  if (typeof window === 'undefined' || !('Notification' in window)) return 'unsupported';
  return Notification.permission;
}

function currentActiveSlug(): string | null {
  if (typeof window === 'undefined') return null;
  const match = window.location.pathname.match(/^\/(?:c|global)\/([^/?#]+)/);
  return match?.[1] ? decodeURIComponent(match[1]) : null;
}

function errorMessage(err: unknown, fallback: string): string {
  return err instanceof Error ? err.message : fallback;
}

export class NotificationStore {
  private state = initialPolicyState();
  private listeners = new Set<() => void>();
  // Live Notification instances retained per conversation id so they can be
  // closed when the conversation is acknowledged without a click.
  private liveByConversationId = new Map<string, Map<string, Notification>>();
  private loadPromise: Promise<NotificationSettings> | null = null;
  // Saves are sequenced so PUTs reach the server in user-edit order; the
  // reducer's `latestSaveId` separately guards which response wins locally.
  private saveChain: Promise<void> = Promise.resolve();

  getState = (): NotificationPolicyState => this.state;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  private emit(): void {
    for (const listener of this.listeners) listener();
  }

  private readEnv(): NotificationEnv {
    const hasDocument = typeof document !== 'undefined';
    return {
      now: Date.now(),
      permission: getBrowserNotificationPermission(),
      visible: hasDocument && document.visibilityState === 'visible',
      hasFocus: hasDocument && document.hasFocus(),
      activeSlug: currentActiveSlug(),
    };
  }

  dispatch = (event: NotificationPolicyEvent): void => {
    const { state, effects } = notificationPolicyReducer(this.state, event, this.readEnv());
    const changed = state !== this.state;
    this.state = state;
    for (const effect of effects) this.runEffect(effect);
    if (changed) this.emit();
  };

  private runEffect(effect: NotificationEffect): void {
    switch (effect.type) {
      case 'load_settings':
        this.loadPromise = api.getNotificationSettings()
          .then((settings) => {
            this.dispatch({ type: 'settings_loaded', settings });
            return settings;
          })
          .catch((err: unknown) => {
            this.loadPromise = null;
            this.dispatch({ type: 'settings_load_failed', error: errorMessage(err, 'Failed to load notification settings') });
            throw err;
          });
        return;

      case 'save_settings': {
        const { requestId, settings } = effect;
        this.saveChain = this.saveChain
          .catch(() => {})
          .then(async () => {
            try {
              const saved = await api.updateNotificationSettings(settings);
              this.dispatch({ type: 'settings_save_succeeded', requestId, settings: saved });
            } catch (err) {
              this.dispatch({ type: 'settings_save_failed', requestId, error: errorMessage(err, 'Failed to save notification settings') });
            }
          });
        return;
      }

      case 'show_browser_notification':
        try {
          const notification = new Notification(effect.title, { body: effect.body, tag: effect.tag });
          this.rememberLive(effect.conversation.id, effect.tag, notification);
          notification.onclick = () => {
            window.focus();
            this.closeForConversation(effect.conversation.id);
            window.dispatchEvent(
              new CustomEvent('phoenix:navigate-to-conversation', {
                detail: { route: notificationRoute(effect.conversation) },
              }),
            );
          };
        } catch (err) {
          if (import.meta.env.DEV && typeof process === 'undefined') {
            console.debug('Failed to create browser notification', err);
          }
          this.dispatch({
            type: 'notification_delivery_failed',
            conversationId: effect.conversation.id,
            attentionKey: effect.attentionKey,
          });
        }
        return;
    }
  }

  // --- Promise-returning facade methods (used by React + DesktopLayout) ---

  loadSettings(): Promise<NotificationSettings> {
    if (this.state.settingsStatus === 'loaded') return Promise.resolve(this.state.settings);
    if (this.loadPromise) return this.loadPromise;
    this.dispatch({ type: 'settings_load_started' });
    return this.loadPromise ?? Promise.resolve(this.state.settings);
  }

  consumePermissionCue(): boolean {
    const pending = this.state.permissionCuePending;
    if (pending) this.dispatch({ type: 'permission_cue_consumed' });
    return pending;
  }

  // --- Live notification registry ---

  private rememberLive(conversationId: string, tag: string, notification: Notification): void {
    let byTag = this.liveByConversationId.get(conversationId);
    if (!byTag) {
      byTag = new Map<string, Notification>();
      this.liveByConversationId.set(conversationId, byTag);
    }
    const previous = byTag.get(tag);
    if (previous && previous !== notification) {
      previous.close();
      // previous.close() fires its onclose -> forgetLive, which removes the
      // now-empty per-conversation map. Re-attach it before storing the
      // replacement so closeForConversation can still find it.
      this.liveByConversationId.set(conversationId, byTag);
    }
    byTag.set(tag, notification);
    notification.onclose = () => this.forgetLive(conversationId, tag, notification);
  }

  private forgetLive(conversationId: string, tag: string, notification: Notification): void {
    const byTag = this.liveByConversationId.get(conversationId);
    if (!byTag || byTag.get(tag) !== notification) return;
    byTag.delete(tag);
    if (byTag.size === 0) this.liveByConversationId.delete(conversationId);
  }

  closeForConversation(conversationId: string): void {
    const byTag = this.liveByConversationId.get(conversationId);
    if (!byTag) return;
    this.liveByConversationId.delete(conversationId);
    for (const notification of byTag.values()) notification.close();
  }

  resetForTest(settings?: NotificationSettings): void {
    this.state = initialPolicyState();
    if (settings) {
      this.state = { ...this.state, settings, settingsStatus: 'loaded' };
    }
    this.liveByConversationId.clear();
    this.loadPromise = null;
    this.saveChain = Promise.resolve();
  }
}
