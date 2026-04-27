import { createContext, useContext } from 'react';

export type Theme = 'dark' | 'light';

export interface ThemeContextValue {
  theme: Theme;
  toggleTheme: () => void;
}

export const THEME_STORAGE_KEY = 'phoenix-theme';

export function getSystemTheme(): Theme {
  if (window.matchMedia?.('(prefers-color-scheme: light)').matches) {
    return 'light';
  }
  return 'dark';
}

export function getInitialTheme(): Theme {
  const stored = localStorage.getItem(THEME_STORAGE_KEY);
  if (stored === 'dark' || stored === 'light') {
    return stored;
  }
  return getSystemTheme();
}

const defaultThemeValue: ThemeContextValue = { theme: 'dark', toggleTheme: () => {} };
export const ThemeContext = createContext<ThemeContextValue>(defaultThemeValue);

export function useTheme(): ThemeContextValue {
  return useContext(ThemeContext);
}
