const admittedTouchStop: unique symbol = Symbol('admittedTouchStop');

export interface OwnedTouchStop {
  readonly [admittedTouchStop]: true;
  readonly event: TouchEvent;
  readonly type: 'touchEnded' | 'touchCancelled';
  readonly identifiers: readonly number[];
  readonly remainingIdentifiers: readonly number[];
  readonly target: EventTarget;
  readonly generation: number;
}

type Registration = { target: EventTarget; stop: EventListener; generation: number };

export class OwnedTouchTargets {
  private registrations = new Map<number, Registration>();
  private ownedTargets = new Map<number, EventTarget>();
  private generation = 0;
  private owner: string | undefined;
  private scroller: HTMLElement | null = null;
  private seen = new WeakSet<TouchEvent>();

  constructor(private readonly receive: (receipt: OwnedTouchStop) => void) {}

  bind(scroller: HTMLElement, owner: string | undefined): void {
    if (this.scroller === scroller && this.owner === owner) return;
    this.clear();
    this.scroller = scroller;
    this.owner = owner;
  }

  clear(): void {
    this.generation++;
    for (const id of Array.from(this.registrations.keys())) this.release(id);
    this.ownedTargets.clear();
    this.scroller = null;
    this.owner = undefined;
  }

  private release(id: number): void {
    const registration = this.registrations.get(id);
    if (!registration) return;
    this.registrations.delete(id);
    for (const type of ['touchend', 'touchcancel']) {
      try {
        registration.target.removeEventListener(type, registration.stop, true);
      } catch (error) {
        console.error('owned-touch listener cleanup failed', { type, identifier: id, generation: registration.generation, error });
      }
    }
  }

  stop(event: TouchEvent): boolean {
    if (!event.isTrusted || !this.scroller || !event.target) return false;
    if (this.seen.has(event)) return true;
    if (!Array.from(this.ownedTargets.values()).includes(event.target)) return false;
    const identifiers = Array.from(event.changedTouches).filter(touch =>
      this.ownedTargets.get(touch.identifier) === (touch.target ?? event.target),
    ).map(touch => touch.identifier);
    if (identifiers.length === 0) return false;
    this.seen.add(event);
    for (const id of identifiers) { this.ownedTargets.delete(id); this.release(id); }
    const live = new Set(Array.from(event.touches).map(touch => touch.identifier));
    for (const id of Array.from(this.ownedTargets.keys())) {
      if (!live.has(id)) { this.ownedTargets.delete(id); this.release(id); }
    }
    try {
      this.receive({ [admittedTouchStop]: true, event, identifiers,
        remainingIdentifiers: Array.from(this.ownedTargets.keys()), target: event.target,
        generation: this.generation, type: event.type === 'touchend' ? 'touchEnded' : 'touchCancelled' });
    } catch (error) {
      console.error('owned-touch receipt delivery failed', { identifiers, generation: this.generation, error });
      throw error;
    }
    return true;
  }

  start(event: TouchEvent): void {
    if (!event.isTrusted || !this.scroller || !event.target
      || !event.composedPath().includes(this.scroller)) return;
    const live = new Set(Array.from(event.touches).map(touch => touch.identifier));
    for (const id of Array.from(this.ownedTargets.keys())) {
      if (!live.has(id)) { this.ownedTargets.delete(id); this.release(id); }
    }
    for (const touch of Array.from(event.changedTouches)) {
      if (!live.has(touch.identifier)) continue;
      const id = touch.identifier;
      this.release(id);
      const generation = this.generation;
      const target = touch.target ?? event.target;
      if (!event.composedPath().includes(target)) continue;
      this.ownedTargets.set(id, target);
      const stop: EventListener = raw => {
        const end = raw as TouchEvent;
        if (!end.isTrusted || generation !== this.generation || end.target !== target
          || this.registrations.get(id)?.stop !== stop) return;
        this.stop(end);
      };
      this.registrations.set(id, { target, stop, generation });
      try {
        target.addEventListener('touchend', stop, { capture: true, passive: true });
        target.addEventListener('touchcancel', stop, { capture: true, passive: true });
      } catch (error) {
        this.release(id);
        console.error('owned-touch listener installation failed', { identifier: id, generation, error });
      }
    }
  }
}
