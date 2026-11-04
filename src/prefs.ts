import type { Language } from "./types";

export type ThemeChoice = "system" | "light" | "dark";

const THEME_KEY = "moby.theme";
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
export const loadLanguage = () => parseLanguage(read(LANGUAGE_KEY));
export const saveLanguage = (language: Language) => write(LANGUAGE_KEY, language);

/** The tokens use light-dark(), so the choice only has to pick the root color-scheme. */
export function applyTheme(theme: ThemeChoice): void {
  if (theme === "system") document.documentElement.removeAttribute("data-theme");
  else document.documentElement.setAttribute("data-theme", theme);
}

export function saveTheme(theme: ThemeChoice): void {
  applyTheme(theme);
  write(THEME_KEY, theme);
}
