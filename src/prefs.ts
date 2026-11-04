import type { Language } from "./types";

export type ThemeChoice = "system" | "light" | "dark";

/** The theme is a per-viewer convenience and must apply before the first paint, so it stays in localStorage. */
const THEME_KEY = "moby.theme";
/** The reply language now lives in the Rust settings. This key only carries an older choice over, or covers a history file that won't open. */
const LANGUAGE_KEY = "moby.language";

export function parseTheme(value: string | null): ThemeChoice {
  return value === "light" || value === "dark" ? value : "system";
}

export function parseLanguage(value: string | null): Language {
  return value === "ru" || value === "en" ? value : "uz";
}

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // The choice still applies until the app closes.
  }
}

export const loadTheme = () => parseTheme(read(THEME_KEY));

/** A language chosen before settings moved to Rust, or while they could not be saved. Null when there is none. */
export function loadSavedLanguage(): Language | null {
  const value = read(LANGUAGE_KEY);
  return value === null ? null : parseLanguage(value);
}

export const saveLanguageFallback = (language: Language) => write(LANGUAGE_KEY, language);

export function clearSavedLanguage(): void {
  try {
    localStorage.removeItem(LANGUAGE_KEY);
  } catch {
    // Nothing to clear.
  }
}

/** The tokens use light-dark(), so the choice only has to pick the root color-scheme. */
export function applyTheme(theme: ThemeChoice): void {
  if (theme === "system") document.documentElement.removeAttribute("data-theme");
  else document.documentElement.setAttribute("data-theme", theme);
}

export function saveTheme(theme: ThemeChoice): void {
  applyTheme(theme);
  write(THEME_KEY, theme);
}
