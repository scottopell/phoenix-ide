import { afterEach, describe, expect, it } from 'vitest';
import {
  ensureTargetTopVisible,
  visibleJumpTop,
} from './jumpScroll';

function rect(top: number, bottom: number, width = 100): DOMRect {
  return {
    top,
    bottom,
    left: 0,
    right: width,
    width,
    height: bottom - top,
    x: 0,
    y: top,
    toJSON: () => ({}),
  } as DOMRect;
}

describe('jumpScroll', () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it('uses the lower edge of the visible nav strip as the jump top', () => {
    const nav = document.createElement('div');
    nav.id = 'conversation-nav';
    nav.getBoundingClientRect = () => rect(0, 36);
    document.body.append(nav);

    const scroller = document.createElement('div');
    scroller.getBoundingClientRect = () => rect(36, 400);
    document.body.append(scroller);

    expect(visibleJumpTop(scroller)).toBe(44);
  });

  it('ignores breadcrumb bars below the message scroller', () => {
    const scroller = document.createElement('div');
    scroller.getBoundingClientRect = () => rect(36, 400);
    document.body.append(scroller);

    const breadcrumb = document.createElement('div');
    breadcrumb.id = 'breadcrumb-bar';
    breadcrumb.getBoundingClientRect = () => rect(400, 436);
    document.body.append(breadcrumb);

    expect(visibleJumpTop(scroller)).toBe(44);
  });

  it('moves scrollTop only when the target top would be hidden by the nav strip', () => {
    const nav = document.createElement('div');
    nav.id = 'conversation-nav';
    nav.getBoundingClientRect = () => rect(0, 36);
    document.body.append(nav);

    const scroller = document.createElement('div');
    scroller.scrollTop = 100;
    scroller.getBoundingClientRect = () => rect(36, 400);
    document.body.append(scroller);

    const target = document.createElement('div');
    target.getBoundingClientRect = () => rect(20 + (100 - scroller.scrollTop), 80 + (100 - scroller.scrollTop));

    expect(ensureTargetTopVisible(target, scroller)).toBe(true);
    expect(scroller.scrollTop).toBe(76);
    expect(ensureTargetTopVisible(target, scroller)).toBe(false);
    expect(scroller.scrollTop).toBe(76);
  });
});
