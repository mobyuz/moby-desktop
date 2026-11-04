import { useState } from "react";
import { asFailure } from "../api";
import { RETENTION_CHOICES } from "../history";
import { loadTheme, saveTheme, type ThemeChoice } from "../prefs";
import { useApp } from "../store";
import type { Language } from "../types";

const THEMES: { value: ThemeChoice; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

const LANGUAGES: { value: Language; label: string }[] = [
  { value: "uz", label: "Uzbek" },
  { value: "ru", label: "Russian" },
  { value: "en", label: "English" },
];

/** Settings belong to this computer, so this page also works before signing in. */
export function Settings() {
  const app = useApp();
  const [theme, setTheme] = useState(loadTheme);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);

  // A value set outside this list (it can't be from this page) still shows instead of looking like another choice.
  const choices = RETENTION_CHOICES.some((item) => item.days === app.retentionDays)
    ? RETENTION_CHOICES
    : [...RETENTION_CHOICES, { days: app.retentionDays, label: `${app.retentionDays} days` }];

  function attempt<T>(action: Promise<T>, done: string | ((result: T) => string)) {
    setError(null);
    setNotice(null);
    action
      .then((result) => setNotice(typeof done === "string" ? done : done(result)))
      .catch((failure: unknown) => setError(asFailure(failure).message));
  }

  return (
    <main className="page">
      <div className="page-column">
        <section>
          <h2>Preferences</h2>
          <div className="field">
            <span id="theme-label">Appearance</span>
            <div className="segmented" role="group" aria-labelledby="theme-label">
              {THEMES.map((item) => (
                <button
                  key={item.value}
                  type="button"
                  aria-pressed={theme === item.value}
                  onClick={() => {
                    setTheme(item.value);
                    saveTheme(item.value);
                  }}
                >
                  {item.label}
                </button>
              ))}
            </div>
          </div>
          <label>
            Moby replies in
            <select value={app.language} onChange={(e) => void app.setLanguage(e.target.value as Language)}>
              {LANGUAGES.map((item) => (
                <option key={item.value} value={item.value}>
                  {item.label}
                </option>
              ))}
            </select>
          </label>
        </section>

        <section>
          <h2>Memory and privacy</h2>
          <p>
            Moby saves your conversations on this computer only, so you can pick them up after a restart. The Moby server does not keep them. They sit in your Windows user profile and are protected by your
            Windows account, not by a separate password.
          </p>
          <label>
            Keep conversations for
            <select
              value={app.retentionDays === null ? "forever" : String(app.retentionDays)}
              onChange={(e) => {
                const days = e.target.value === "forever" ? null : Number(e.target.value);
                attempt(app.setRetention(days), (removed: number) =>
                  removed === 0 ? "Saved." : `Saved. Removed ${removed} older ${removed === 1 ? "conversation" : "conversations"}.`,
                );
              }}
            >
              {choices.map((item) => (
                <option key={item.days ?? "forever"} value={item.days === null ? "forever" : String(item.days)}>
                  {item.label}
                </option>
              ))}
            </select>
          </label>
          <p className="hint">Older conversations are removed each time Moby starts and when you change this.</p>
          {!confirming ? (
            <div className="row">
              <button className="danger" type="button" onClick={() => setConfirming(true)}>
                Delete all history
              </button>
            </div>
          ) : (
            <div className="delete-form">
              <p>This deletes every saved conversation on this computer, for every account. It can't be undone.</p>
              <div className="row">
                <button
                  className="danger"
                  type="button"
                  onClick={() => {
                    setConfirming(false);
                    attempt(app.deleteAllHistory(), "All history on this computer was deleted.");
                  }}
                >
                  Delete everything
                </button>
                <button className="secondary" type="button" onClick={() => setConfirming(false)}>
                  Cancel
                </button>
              </div>
            </div>
          )}
          {notice && (
            <p className="ok" role="status">
              {notice}
            </p>
          )}
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
        </section>

        <section>
          <h2>Activity log</h2>
          <p className="muted">Once Moby can do things on your computer, it will list what it did here, without the contents. Nothing is recorded yet.</p>
          <div className="row">
            <button className="secondary" type="button" disabled>
              Clear activity log
            </button>
          </div>
        </section>
      </div>
    </main>
  );
}
