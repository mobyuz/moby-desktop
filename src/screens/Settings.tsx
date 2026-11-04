import { useApp } from "../store";
import type { SpeechLanguage } from "../types";

const LANGUAGES: { value: SpeechLanguage; label: string }[] = [
  { value: "uz", label: "Uzbek" },
  { value: "ru", label: "Russian" },
  { value: "en", label: "English" },
];

export function Settings() {
  const app = useApp();
  const { settings } = app;

  return (
    <div className="stack">
      <section className="card form-grid">
        <h2>Voice</h2>
        <label>
          Speech language
          <select value={settings.speechLanguage} onChange={(e) => app.updateSettings({ speechLanguage: e.target.value as SpeechLanguage })}>
            {LANGUAGES.map((item) => (
              <option key={item.value} value={item.value}>
                {item.label}
              </option>
            ))}
          </select>
        </label>
        <label className="check">
          <input type="checkbox" checked={settings.speakReplies} onChange={(e) => app.updateSettings({ speakReplies: e.target.checked })} />
          <span>Speak replies aloud (needs cloud voice consent)</span>
        </label>
        <label>
          Wake word sensitivity: {settings.wakeSensitivity}%
          <input
            type="range"
            min={0}
            max={100}
            value={settings.wakeSensitivity}
            onChange={(e) => app.updateSettings({ wakeSensitivity: Number(e.target.value) })}
          />
          <span className="hint">Applies to "Hey Moby" once the robot firmware supports it. Higher values wake more easily and may wake by mistake.</span>
        </label>
      </section>

      <section className="card form-grid">
        <h2>Interface</h2>
        <label>
          Language
          <select value="en" disabled>
            <option value="en">English</option>
          </select>
          <span className="hint">More interface languages come later.</span>
        </label>
        <p className="hint">Telemetry is off. Crash reports are only sent if you agree.</p>
      </section>
    </div>
  );
}
