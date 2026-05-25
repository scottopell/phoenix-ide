import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { UnitHeightCache } from './unitHeightCache';

describe('UnitHeightCache', () => {
  beforeEach(() => {
    sessionStorage.clear();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    sessionStorage.clear();
  });

  describe('in-memory operations', () => {
    it('stores and retrieves heights by key', () => {
      const cache = new UnitHeightCache('conv-1');
      cache.set('u1', 120);
      cache.set('u2', 80);
      expect(cache.get('u1')).toBe(120);
      expect(cache.get('u2')).toBe(80);
      expect(cache.get('missing')).toBeUndefined();
    });

    it('overwrites an existing entry', () => {
      const cache = new UnitHeightCache('conv-1');
      cache.set('u1', 100);
      cache.set('u1', 200);
      expect(cache.get('u1')).toBe(200);
    });

    it('does not bump the version when set with an unchanged value', () => {
      const cache = new UnitHeightCache('conv-1');
      cache.set('u1', 100);
      const v1 = cache.version;
      cache.set('u1', 100);
      expect(cache.version).toBe(v1);
    });

    it('bumps the version on each mutating set', () => {
      const cache = new UnitHeightCache('conv-1');
      const v0 = cache.version;
      cache.set('u1', 100);
      cache.set('u2', 50);
      expect(cache.version).toBeGreaterThan(v0);
      expect(cache.version).toBe(v0 + 2);
    });
  });

  describe('subscribers', () => {
    it('notifies on each mutating set', () => {
      const cache = new UnitHeightCache('conv-1');
      const listener = vi.fn();
      const unsub = cache.subscribe(listener);
      cache.set('u1', 100);
      cache.set('u2', 50);
      expect(listener).toHaveBeenCalledTimes(2);
      unsub();
      cache.set('u3', 25);
      expect(listener).toHaveBeenCalledTimes(2);
    });

    it('does not notify when set with an unchanged value', () => {
      const cache = new UnitHeightCache('conv-1');
      cache.set('u1', 100);
      const listener = vi.fn();
      cache.subscribe(listener);
      cache.set('u1', 100);
      expect(listener).not.toHaveBeenCalled();
    });
  });

  describe('sessionStorage mirror', () => {
    it('persists writes to sessionStorage after the flush timer', () => {
      const cache = new UnitHeightCache('conv-1');
      cache.set('u1', 120);
      // Pending — not yet flushed
      expect(sessionStorage.getItem('phoenix:hcache:conv-1:u1')).toBeNull();
      vi.advanceTimersByTime(20);
      expect(sessionStorage.getItem('phoenix:hcache:conv-1:u1')).toBe('120');
    });

    it('coalesces multiple writes within the flush window', () => {
      const cache = new UnitHeightCache('conv-1');
      cache.set('u1', 100);
      cache.set('u2', 200);
      cache.set('u1', 150); // overwrites pending
      vi.advanceTimersByTime(20);
      expect(sessionStorage.getItem('phoenix:hcache:conv-1:u1')).toBe('150');
      expect(sessionStorage.getItem('phoenix:hcache:conv-1:u2')).toBe('200');
    });

    it('flush() writes synchronously without waiting for the timer', () => {
      const cache = new UnitHeightCache('conv-1');
      cache.set('u1', 99);
      cache.flush();
      expect(sessionStorage.getItem('phoenix:hcache:conv-1:u1')).toBe('99');
    });

    it('hydrates existing sessionStorage entries for the conversation on construct', () => {
      sessionStorage.setItem('phoenix:hcache:conv-1:u1', '111');
      sessionStorage.setItem('phoenix:hcache:conv-1:u2', '222');
      sessionStorage.setItem('phoenix:hcache:other:u1', '999');
      const cache = new UnitHeightCache('conv-1');
      expect(cache.get('u1')).toBe(111);
      expect(cache.get('u2')).toBe(222);
      // Entries from a different conversation are not visible.
      expect(cache.get('u3')).toBeUndefined();
    });

    it('skips malformed entries during hydration', () => {
      sessionStorage.setItem('phoenix:hcache:conv-1:u1', 'not-a-number');
      sessionStorage.setItem('phoenix:hcache:conv-1:u2', '50');
      const cache = new UnitHeightCache('conv-1');
      expect(cache.get('u1')).toBeUndefined();
      expect(cache.get('u2')).toBe(50);
    });

    it('does not touch sessionStorage when conversationId is undefined', () => {
      const cache = new UnitHeightCache(undefined);
      cache.set('u1', 100);
      vi.advanceTimersByTime(20);
      expect(sessionStorage.length).toBe(0);
      // In-memory cache still works.
      expect(cache.get('u1')).toBe(100);
    });
  });

  describe('clearConversation', () => {
    it('removes only the matching conversation entries', () => {
      sessionStorage.setItem('phoenix:hcache:conv-1:u1', '100');
      sessionStorage.setItem('phoenix:hcache:conv-1:u2', '200');
      sessionStorage.setItem('phoenix:hcache:conv-2:u1', '300');
      sessionStorage.setItem('unrelated', 'keep');
      UnitHeightCache.clearConversation('conv-1');
      expect(sessionStorage.getItem('phoenix:hcache:conv-1:u1')).toBeNull();
      expect(sessionStorage.getItem('phoenix:hcache:conv-1:u2')).toBeNull();
      expect(sessionStorage.getItem('phoenix:hcache:conv-2:u1')).toBe('300');
      expect(sessionStorage.getItem('unrelated')).toBe('keep');
    });
  });
});
