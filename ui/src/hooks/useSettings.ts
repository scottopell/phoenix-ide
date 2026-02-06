import { useState, useEffect, useCallback } from 'react';

export interface Settings {
  showPerformanceDashboard: boolean;
  showLayoutOverlay: boolean;
  theme: 'light' | 'dark';
}

const SETTINGS_KEY = 'phoenix-settings';

const defaultSettings: Settings = {
  showPerformanceDashboard: false,
  showLayoutOverlay: false,
  theme: 'dark',
};

function loadSettings(): Settings {
  try {
    const stored = localStorage.getItem(SETTINGS_KEY);
    if (stored) {
      return { ...defaultSettings, ...JSON.parse(stored) };
    }
  } catch (e) {
    console.error('Failed to load settings:', e);
  }
  return defaultSettings;
}

function saveSettings(settings: Settings): void {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
  } catch (e) {
    console.error('Failed to save settings:', e);
  }
}

// Simple global state for settings
let globalSettings = loadSettings();
const listeners = new Set<() => void>();

function notifyListeners() {
  listeners.forEach(fn => fn());
}

export function useSettings() {
  const [settings, setSettingsState] = useState<Settings>(globalSettings);

  useEffect(() => {
    const listener = () => setSettingsState({ ...globalSettings });
    listeners.add(listener);
    return () => { listeners.delete(listener); };
  }, []);

  const updateSettings = useCallback((updates: Partial<Settings>) => {
    globalSettings = { ...globalSettings, ...updates };
    saveSettings(globalSettings);
    notifyListeners();
  }, []);

  return { settings, updateSettings };
}
