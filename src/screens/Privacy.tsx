import { useState } from "react";
import { useApp } from "../store";
import type { ConsentKind } from "../types";

const CONSENTS: { kind: ConsentKind; label: string; detail: string }[] = [
  { kind: "terms", label: "Terms of use", detail: "Required to use your account." },
  { kind: "privacy_policy", label: "Privacy policy", detail: "Required to use your account." },
  { kind: "cloud_voice", label: "Cloud voice", detail: "Your voice is sent to the server to be turned into text and is not stored." },
];

export function Privacy() {
  const app = useApp();
  const [confirm, setConfirm] = useState(false);
  const real = app.messages.filter((item) => item.id !== "welcome");

  return (
    <div className="stack">
      <section className="card">
        <h2>Consent</h2>
        {!app.profile && <p className="muted">Sign in to manage consent.</p>}
        <ul className="list">
          {CONSENTS.map(({ kind, label, detail }) => {
            const record = app.consents.find((item) => item.type === kind && item.revoked_at === null);
            const required = kind !== "cloud_voice";
            return (
              <li key={kind} className="row between">
                <span>
                  <strong>{label}</strong>
                  <span className="muted block">
                    {detail}
                    {record ? ` Accepted version ${record.version}.` : ""}
                  </span>
                </span>
                <label className="switch">
                  <input
                    type="checkbox"
                    role="switch"
                    checked={Boolean(record)}
                    disabled={!app.profile || required}
                    onChange={(e) => void app.setConsent(kind, e.target.checked)}
                    aria-label={`${label} consent`}
                  />
                  <span>{record ? "Granted" : "Off"}</span>
                </label>
              </li>
            );
          })}
        </ul>
      </section>

      <section className="card">
        <h2>Conversation history</h2>
        <p className="hint">Stored only on this computer. The server keeps no conversation content.</p>
        <div className="row between">
          <span>{real.length} messages · {app.approvals.length} tool decisions</span>
          <label className="row">
            Delete automatically after
            <select value={app.settings.autoDeleteDays} onChange={(e) => app.updateSettings({ autoDeleteDays: Number(e.target.value) })}>
              {[7, 30, 90, 365].map((days) => (
                <option key={days} value={days}>
                  {days} days
                </option>
              ))}
            </select>
          </label>
        </div>
        <ul className="list history">
          {real.slice(-6).map((message) => (
            <li key={message.id}>
              <span className="muted">{message.role === "user" ? "You" : "Moby"}:</span> {message.text}
            </li>
          ))}
          {real.length === 0 && <li className="muted">No history yet.</li>}
        </ul>
        {!confirm ? (
          <button className="danger" type="button" onClick={() => setConfirm(true)} disabled={real.length === 0 && app.approvals.length === 0}>
            Delete all history…
          </button>
        ) : (
          <div className="row">
            <span>Delete everything stored here?</span>
            <button
              className="danger"
              type="button"
              onClick={() => {
                app.clearHistory();
                setConfirm(false);
              }}
            >
              Yes, delete it
            </button>
            <button className="secondary" type="button" onClick={() => setConfirm(false)}>
              Cancel
            </button>
          </div>
        )}
      </section>
    </div>
  );
}
