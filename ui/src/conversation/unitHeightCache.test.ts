import { describe, it, expect, vi } from 'vitest';
import { UnitHeightCache } from './unitHeightCache';

describe('UnitHeightCache', () => {
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

    it('works with an undefined conversationId', () => {
      const cache = new UnitHeightCache(undefined);
      cache.set('u1', 100);
      expect(cache.get('u1')).toBe(100);
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

  describe('dispose', () => {
    it('clears the in-memory map and listeners', () => {
      const cache = new UnitHeightCache('conv-1');
      cache.set('u1', 100);
      const listener = vi.fn();
      cache.subscribe(listener);
      cache.dispose();
      expect(cache.get('u1')).toBeUndefined();
      cache.set('u1', 200);
      expect(listener).not.toHaveBeenCalled();
    });
  });
});
