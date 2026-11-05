import { useState, type FormEvent } from "react";
import { api, asFailure } from "../api";
import { useApp } from "../store";

export function Account() {
  const app = useApp();
  const [name, setName] = useState(app.profile?.display_name ?? "");
  const [saved, setSaved] = useState(false);
  const [exportPath, setExportPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [password, setPassword] = useState("");
  const [deleteError, setDeleteError] = useState<string | null>(null);

  const attempt = (action: Promise<unknown>) => {
    setError(null);
    action.catch((failure: unknown) => setError(asFailure(failure).message));
  };

  const saveName = (event: FormEvent) => {
    event.preventDefault();
    setSaved(false);
    attempt(app.saveName(name).then(() => setSaved(true)));
  };

  const deleteAccount = (event: FormEvent) => {
    event.preventDefault();
    setDeleteError(null);
    app.deleteAccount(password).catch((failure: unknown) => setDeleteError(asFailure(failure).message));
  };

  const profile = app.profile;
  if (!profile) return null;

  return (
    <main className="page">
      <div className="page-column">
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}

        <section>
          <h2>Profile</h2>
          <p className="muted">{profile.email}</p>
          <form className="row" onSubmit={saveName}>
            <label className="grow">
              Name
              <input
                value={name}
                maxLength={100}
                onChange={(e) => {
                  setSaved(false);
                  setName(e.target.value);
                }}
              />
            </label>
            <button className="secondary" type="submit">
              Save
            </button>
            {saved && <span className="ok">Saved</span>}
          </form>
        </section>

        <section>
          <h2>Usage today</h2>
          {app.usage ? (
            <p>
              <strong>{app.usage.llm_tokens_remaining.toLocaleString("en")}</strong> tokens left
              <span className="muted">
                {" · resets "}
                {new Date(app.usage.reset_at).toLocaleString("en", { dateStyle: "medium", timeStyle: "short" })}
              </span>
            </p>
          ) : (
            <p className="muted">Not available right now.</p>
          )}
        </section>

        <section>
          <h2>Signed-in computers</h2>
          <ul className="list">
            {app.installations
              .filter((item) => !item.revoked)
              .map((item) => (
                <li key={item.id} className="row between">
                  <span>
                    <strong>{item.name}</strong>
                    {item.id === profile.installation_id && <span className="tag">This computer</span>}
                  </span>
                  {item.id !== profile.installation_id && (
                    <button className="secondary small" type="button" onClick={() => attempt(app.revokeInstallation(item.id))}>
                      Sign out
                    </button>
                  )}
                </li>
              ))}
          </ul>
        </section>

        <section>
          <h2>Your account</h2>
          <div className="row">
            <button className="secondary" type="button" onClick={() => attempt(api.exportAccount().then(setExportPath))}>
              Download my data
            </button>
            <button className="secondary" type="button" onClick={() => void app.signOut()}>
              Sign out
            </button>
            {!deleting && (
              <button className="danger" type="button" onClick={() => setDeleting(true)}>
                Delete account
              </button>
            )}
          </div>
          {exportPath && <p className="ok">Saved to {exportPath}</p>}
          {deleting && (
            <form className="delete-form" onSubmit={deleteAccount}>
              <p>This deletes your account for good.</p>
              <div className="row">
                <label className="grow">
                  Password
                  <input type="password" autoComplete="current-password" value={password} minLength={15} onChange={(e) => setPassword(e.target.value)} required />
                </label>
                <button className="danger" type="submit">
                  Delete forever
                </button>
                <button className="secondary" type="button" onClick={() => setDeleting(false)}>
                  Cancel
                </button>
              </div>
              {deleteError && (
                <p className="error" role="alert">
                  {deleteError}
                </p>
              )}
            </form>
          )}
        </section>
      </div>
    </main>
  );
}
