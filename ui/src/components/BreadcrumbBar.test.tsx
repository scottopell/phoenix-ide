import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { BreadcrumbBar } from './BreadcrumbBar';
import { calcTooltipPosition } from './breadcrumbTooltipPosition';
import type { Breadcrumb } from '../types';

const breadcrumbs: Breadcrumb[] = [
  { type: 'user', label: 'User', sequenceId: 1, preview: 'Asked a question' },
  {
    type: 'tool',
    label: 'bash',
    toolId: 'tool-1',
    sequenceId: 2,
    resultSummary: 'pwd returned /tmp/project',
  },
];

describe('BreadcrumbBar', () => {
  afterEach(() => {
    document.querySelectorAll('#messages').forEach((el) => el.remove());
  });

  it('uses accessible labels instead of native title tooltips', () => {
    render(<BreadcrumbBar breadcrumbs={breadcrumbs} visible />);

    const toolBreadcrumb = screen.getByLabelText('bash: Running a tool');
    expect(toolBreadcrumb).toHaveClass('breadcrumb-item', 'tool');
    expect(toolBreadcrumb).not.toHaveAttribute('title');
  });

  it('renders rich tooltip through a body portal on hover', async () => {
    vi.useFakeTimers();
    try {
      render(<BreadcrumbBar breadcrumbs={breadcrumbs} visible />);

      const bar = document.querySelector<HTMLElement>('#breadcrumb-bar');
      expect(bar).toBeInTheDocument();

      const toolBreadcrumb = screen.getByLabelText('bash: Running a tool');
      vi.spyOn(toolBreadcrumb, 'getBoundingClientRect').mockReturnValue({
        left: 320,
        right: 380,
        top: 640,
        bottom: 664,
        width: 60,
        height: 24,
        x: 320,
        y: 640,
        toJSON: () => ({}),
      } as DOMRect);

      fireEvent.mouseEnter(toolBreadcrumb);
      act(() => {
        vi.advanceTimersByTime(150);
      });

      expect(screen.getByText('pwd returned /tmp/project')).toBeInTheDocument();

      const tooltip = screen.getByText('pwd returned /tmp/project').closest('.breadcrumb-tooltip');
      expect(tooltip).not.toBeNull();
      expect(tooltip!.parentElement).toBe(document.body);
      expect(within(bar!).queryByText('pwd returned /tmp/project')).not.toBeInTheDocument();
      expect(tooltip).toHaveStyle({ top: '632px', transform: 'translateY(-100%)' });
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps clicked breadcrumbs visible below the breadcrumb strip', () => {
    vi.useFakeTimers();
    const originalScrollIntoView = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = vi.fn();
    try {
      const scroller = document.createElement('div');
      scroller.id = 'messages';
      scroller.scrollTop = 100;
      scroller.getBoundingClientRect = () => ({
        left: 0,
        right: 800,
        top: 36,
        bottom: 600,
        width: 800,
        height: 564,
        x: 0,
        y: 36,
        toJSON: () => ({}),
      } as DOMRect);
      document.body.append(scroller);

      const target = document.createElement('div');
      target.dataset['sequenceId'] = '2';
      target.getBoundingClientRect = () => ({
        left: 0,
        right: 800,
        top: 20 + (100 - scroller.scrollTop),
        bottom: 80 + (100 - scroller.scrollTop),
        width: 800,
        height: 60,
        x: 0,
        y: 20 + (100 - scroller.scrollTop),
        toJSON: () => ({}),
      } as DOMRect);
      scroller.append(target);

      render(<BreadcrumbBar breadcrumbs={breadcrumbs} visible />);
      const bar = document.querySelector<HTMLElement>('#breadcrumb-bar')!;
      bar.getBoundingClientRect = () => ({
        left: 0,
        right: 800,
        top: 0,
        bottom: 36,
        width: 800,
        height: 36,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect);

      fireEvent.click(screen.getByLabelText('bash: Running a tool'));
      expect(target).toHaveClass('breadcrumb-highlight');
      expect(Element.prototype.scrollIntoView).toHaveBeenCalledWith({ behavior: 'smooth', block: 'center' });

      act(() => {
        vi.advanceTimersByTime(120);
      });
      expect(scroller.scrollTop).toBe(76);
    } finally {
      Element.prototype.scrollIntoView = originalScrollIntoView;
      vi.useRealTimers();
    }
  });

  it('cancels stale breadcrumb offset retries after a newer click', () => {
    vi.useFakeTimers();
    const originalScrollIntoView = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = vi.fn();
    try {
      const scroller = document.createElement('div');
      scroller.id = 'messages';
      scroller.scrollTop = 100;
      scroller.getBoundingClientRect = () => ({
        left: 0,
        right: 800,
        top: 36,
        bottom: 600,
        width: 800,
        height: 564,
        x: 0,
        y: 36,
        toJSON: () => ({}),
      } as DOMRect);
      document.body.append(scroller);

      const firstTarget = document.createElement('div');
      firstTarget.dataset['sequenceId'] = '1';
      firstTarget.getBoundingClientRect = () => ({
        left: 0,
        right: 800,
        top: 20 + (100 - scroller.scrollTop),
        bottom: 80 + (100 - scroller.scrollTop),
        width: 800,
        height: 60,
        x: 0,
        y: 20 + (100 - scroller.scrollTop),
        toJSON: () => ({}),
      } as DOMRect);
      scroller.append(firstTarget);

      const secondTarget = document.createElement('div');
      secondTarget.dataset['sequenceId'] = '2';
      secondTarget.getBoundingClientRect = () => ({
        left: 0,
        right: 800,
        top: 25 + (100 - scroller.scrollTop),
        bottom: 85 + (100 - scroller.scrollTop),
        width: 800,
        height: 60,
        x: 0,
        y: 25 + (100 - scroller.scrollTop),
        toJSON: () => ({}),
      } as DOMRect);
      scroller.append(secondTarget);

      render(<BreadcrumbBar breadcrumbs={breadcrumbs} visible />);
      const bar = document.querySelector<HTMLElement>('#breadcrumb-bar')!;
      bar.getBoundingClientRect = () => ({
        left: 0,
        right: 800,
        top: 0,
        bottom: 36,
        width: 800,
        height: 36,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect);

      fireEvent.click(screen.getByLabelText('User: Your message'));
      act(() => {
        vi.advanceTimersByTime(60);
      });
      fireEvent.click(screen.getByLabelText('bash: Running a tool'));

      act(() => {
        vi.advanceTimersByTime(601);
      });
      expect(scroller.scrollTop).toBe(81);
    } finally {
      Element.prototype.scrollIntoView = originalScrollIntoView;
      vi.useRealTimers();
    }
  });

  it('positions tooltip above the breadcrumb and clamps to viewport margins', () => {
    const originalInnerWidth = window.innerWidth;
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 320 });

    const position = calcTooltipPosition({
      left: 0,
      right: 40,
      top: 4,
      bottom: 28,
      width: 40,
      height: 24,
      x: 0,
      y: 4,
      toJSON: () => ({}),
    } as DOMRect);

    expect(position).toEqual({
      tooltipLeft: 8,
      tooltipTop: 8,
      arrowLeft: 12,
    });

    Object.defineProperty(window, 'innerWidth', { configurable: true, value: originalInnerWidth });
  });

  it('keeps tooltip left non-negative on very narrow viewports', () => {
    const originalInnerWidth = window.innerWidth;
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 120 });

    const position = calcTooltipPosition({
      left: 44,
      right: 76,
      top: 100,
      bottom: 124,
      width: 32,
      height: 24,
      x: 44,
      y: 100,
      toJSON: () => ({}),
    } as DOMRect);

    expect(position.tooltipLeft).toBe(8);
    expect(position.arrowLeft).toBe(52);

    Object.defineProperty(window, 'innerWidth', { configurable: true, value: originalInnerWidth });
  });
});
