import { useEffect, useState } from "react";
import { DAILY_LIMITS } from "../demo";
import { useApp } from "../store";

function Meter({ label, value, max, unit }: { label: string; value: number; max: number; unit: string }) {
  return (
    <div className="meter-row">
      <div className="row between">
        <span>{label}</span>
        <span className="muted">
          {value.toLocaleString("en")} / {max.toLocaleString("en")} {unit} left
        </span>
      </div>
      <meter min={0} max={max} low={max * 0.2} optimum={max} value={value} aria-label={`${label} remaining`} />
    </div>
  );
}

export function Account({ onSignIn }: { onSignIn: () => void }) {
  const app = useApp();
  const [name, setName] = useState(app.profile?.display_name ?? "");
  const [saved, setSaved] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [password, setPassword] = useState("");
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [exported, setExported] = useState<number | null>(null);

  useEffect(() => setName(app.profile?.display_name ?? ""), [app.profile?.display_name]);

  if (!app.profile) {
    return (
      <section className="card empty">
        <h2>You are not signed in</h2>
        <p className="muted">Profile, usage and installations need an account. The virtual Moby, tools and settings still work.</p>
        <button className="primary" type="button" onClick={onSignIn}>
          Sign in
        </button>
      </section>
    );
  }

  const usage = app.usage;
  const resetAt = usage ? new Date(usage.reset_at) : null;

  return (
    <div className="stack">
      <section className="card">
        <h2>Profile</h2>
        <div className="profile-head">
          <span className="avatar" aria-hidden="true">
            {(app.profile.display_name ?? app.profile.email).slice(0, 1).toUpperCase()}
          </span>
          <div>
            <strong>{app.profile.display_name ?? "No display name"}</strong>
            <p className="muted">{app.profile.email}</p>
          </div>
        </div>
        <form
          className="row"
          onSubmit={(event) => {
            event.preventDefault();
            void app.saveProfileName(name).then(() => setSaved(true));
          }}
        >
          <label className="grow">
            Display name
            <input value={name} maxLength={80} onChange={(e) => {
                setSaved(false);
                setName(e.target.value);
              }} />
          </label>
          <button className="primary" type="submit">
            Save
          </button>
          {saved && <span className="ok">Saved</span>}
        </form>
      </section>

      <section className="card">
        <h2>Usage today</h2>
        {usage ? (
          <>
            <Meter label="Language model" value={usage.llm_tokens_remaining} max={DAILY_LIMITS.llm} unit="tokens" />
            <Meter label="Speech to text" value={usage.stt_seconds_remaining} max={DAILY_LIMITS.stt} unit="s" />
            <Meter label="Text to speech" value={usage.tts_chars_remaining} max={DAILY_LIMITS.tts} unit="chars" />
            <p className="hint">Resets at {resetAt?.toLocaleString("en", { dateStyle: "medium", timeStyle: "short" })} (midnight UTC). Limits shown here are demo values.</p>
          </>
        ) : (
          <p className="muted">Loading…</p>
        )}
      </section>

      <section className="card">
        <h2>Signed-in computers</h2>
        <ul className="list">
          {app.installations.map((item) => (
            <li key={item.id} className="row between">
              <span>
                <strong>{item.name}</strong>
                {item.id === app.profile?.installation_id && <span className="tag">This computer</span>}
                <span className="muted block">
                  {item.os} · app {item.app_version ?? "unknown"}
                </span>
              </span>
              {item.revoked ? (
                <span className="muted">Revoked</span>
              ) : item.id === app.profile?.installation_id ? null : (
                <button className="secondary small" type="button" onClick={() => void app.revokeInstallation(item.id)}>
                  Revoke
                </button>
              )}
            </li>
          ))}
        </ul>
      </section>

      <section className="card">
        <h2>Your data</h2>
        <div className="row">
          <button className="secondary" type="button" onClick={() => setExported(app.backend.exportSize())}>
            Export account data
          </button>
          {exported !== null && <span className="ok">Export prepared ({exported.toLocaleString("en")} bytes, demo)</span>}
          <button className="secondary" type="button" onClick={() => void app.signOut()}>
            Sign out
          </button>
        </div>
        <details className="danger-zone">
          <summary>Delete account</summary>
          {!confirming ? (
            <button className="danger" type="button" onClick={() => setConfirming(true)}>
              Delete my account…
            </button>
          ) : (
            <form
              className="row"
              onSubmit={(event) => {
                event.preventDefault();
                void app.deleteAccount(password).then(setDeleteError);
              }}
            >
              <label className="grow">
                Confirm with your password
                <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} required />
              </label>
              <button className="danger" type="submit">
                Delete forever
              </button>
              {deleteError && <span className="error">{deleteError}</span>}
            </form>
          )}
        </details>
      </section>
    </div>
  );
}
