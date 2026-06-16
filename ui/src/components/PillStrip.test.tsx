import { render } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { PillStrip, type PillItem } from './PillStrip';

const items: PillItem[] = [
  { key: 'one', label: 'One' },
  { key: 'two', label: 'Two' },
  { key: 'three', label: 'Three' },
];

const activeItems: PillItem[] = [
  { key: 'one', label: 'One' },
  { key: 'two', label: 'Two', active: true },
  { key: 'three', label: 'Three' },
];

const originalClientWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');
const originalOffsetLeft = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetLeft');
const originalOffsetWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetWidth');

function restoreLayoutProperty(property: 'clientWidth' | 'offsetLeft' | 'offsetWidth', descriptor: PropertyDescriptor | undefined) {
  if (descriptor) {
    Object.defineProperty(HTMLElement.prototype, property, descriptor);
  } else {
    delete (HTMLElement.prototype as unknown as Record<typeof property, unknown>)[property];
  }
}

function mockLayout({ activeLeft, activeWidth, stripWidth }: { activeLeft: number; activeWidth: number; stripWidth: number }) {
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', {
    configurable: true,
    get() {
      return this.id === 'pill-strip' ? stripWidth : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, 'offsetLeft', {
    configurable: true,
    get() {
      return this.dataset?.['active'] === 'true' ? activeLeft : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, 'offsetWidth', {
    configurable: true,
    get() {
      return this.dataset?.['active'] === 'true' ? activeWidth : 0;
    },
  });
}

describe('PillStrip', () => {
  afterEach(() => {
    restoreLayoutProperty('clientWidth', originalClientWidth);
    restoreLayoutProperty('offsetLeft', originalOffsetLeft);
    restoreLayoutProperty('offsetWidth', originalOffsetWidth);
  });
  it('does not jump when the active pill is already visible', () => {
    mockLayout({ activeLeft: 40, activeWidth: 20, stripWidth: 100 });
    const { rerender } = render(<PillStrip items={items} navId="pill-strip" />);
    const strip = document.querySelector<HTMLElement>('#pill-strip')!;
    strip.scrollLeft = 30;

    rerender(<PillStrip items={activeItems} navId="pill-strip" />);

    expect(strip.scrollLeft).toBe(30);
  });

  it('scrolls horizontally to reveal an active pill outside the visible range', () => {
    mockLayout({ activeLeft: 160, activeWidth: 30, stripWidth: 100 });
    const { rerender } = render(<PillStrip items={items} navId="pill-strip" />);
    const strip = document.querySelector<HTMLElement>('#pill-strip')!;
    strip.scrollLeft = 30;

    rerender(<PillStrip items={activeItems} navId="pill-strip" />);

    expect(strip.scrollLeft).toBe(90);
  });

  it('does not scroll when no item is active', () => {
    mockLayout({ activeLeft: 160, activeWidth: 30, stripWidth: 100 });
    const { rerender } = render(<PillStrip items={items} navId="pill-strip" />);
    const strip = document.querySelector<HTMLElement>('#pill-strip')!;
    strip.scrollLeft = 45;

    rerender(<PillStrip items={[...items]} navId="pill-strip" />);

    expect(strip.scrollLeft).toBe(45);
  });
});
