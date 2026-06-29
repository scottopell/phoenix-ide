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
 * Assistant `text` blocks at or above this many characters are substantial:
 * they always render full in compact mode and become conversation chapters.
 * Shorter compact prose may still render full when its preview would not hide
 * content.
 */
export const SIGNIFICANCE_THRESHOLD = 280;

/** True when an assistant text block is substantial enough to always
 *  render full and become a conversation chapter. */
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
