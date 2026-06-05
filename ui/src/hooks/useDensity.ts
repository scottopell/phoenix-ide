import { createContext, useContext } from 'react';

/**
 * Conversation view density. `full` renders every message exactly as it
 * has always rendered. `compact` makes long conversations skimmable by
 * collapsing each agent turn's tool calls into an inline pill strip and
 * folding short assistant prose into an expandable one-liner. Nothing is
 * ever destroyed — every collapsed element expands on click.
 */
export type Density = 'full' | 'compact';

export interface DensityContextValue {
  density: Density;
  setDensity: (density: Density) => void;
}

export const DENSITY_STORAGE_KEY = 'phoenix-conv-density';

/**
 * Assistant `text` blocks shorter than this many characters are treated
 * as insignificant in compact mode and collapse to a faded one-liner;
 * prose at or above it always renders full. A single named constant so
 * the threshold has one home and can be tuned in one place.
 */
export const SIGNIFICANCE_THRESHOLD = 280;

/** True when an assistant text block is substantial enough to always
 *  render full, even in compact mode. */
export function isSignificantText(text: string): boolean {
  return text.length >= SIGNIFICANCE_THRESHOLD;
}

const defaultDensityValue: DensityContextValue = {
  density: 'full',
  setDensity: () => {},
};

export const DensityContext = createContext<DensityContextValue>(defaultDensityValue);

export function useDensity(): DensityContextValue {
  return useContext(DensityContext);
}
