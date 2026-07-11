import { render, screen, fireEvent } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { FindBar } from './FindBar';

describe('FindBar', () => {
  it('renders an accessible search control and wires commands', () => {
    const onQueryChange = vi.fn();
    const onNext = vi.fn();
    const onPrevious = vi.fn();
    const onClose = vi.fn();

    render(
      <FindBar
        query="alpha"
        activeIndex={1}
        matchCount={3}
        onQueryChange={onQueryChange}
        onNext={onNext}
        onPrevious={onPrevious}
        onClose={onClose}
      />,
    );

    const input = screen.getByRole('textbox', { name: 'Find in viewer' });
    expect(input).toHaveFocus();
    expect(screen.getByText('2 of 3')).toBeInTheDocument();

    fireEvent.change(input, { target: { value: 'beta' } });
    expect(onQueryChange).toHaveBeenCalledWith('beta');

    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onNext).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(input, { key: 'Enter', shiftKey: true });
    expect(onPrevious).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(input, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('disables navigation buttons when there are no matches', () => {
    render(
      <FindBar
        query="missing"
        activeIndex={-1}
        matchCount={0}
        onQueryChange={() => undefined}
        onNext={() => undefined}
        onPrevious={() => undefined}
        onClose={() => undefined}
        autoFocus={false}
      />,
    );

    expect(screen.getByText('0 results')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Previous' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
  });
});
