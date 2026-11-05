import { useCallback, useEffect, useState } from "react";
import { api, asFailure } from "../api";
import { formatWhen } from "../history";
import { activityLine, resourceName, toolLabel } from "../tools";
import type { ActivityEntry, Registry } from "../types";

/** What Moby did or was stopped from doing. Names and decisions only: never file names, queries or message text. */
export function Activity() {
  const [entries, setEntries] = useState<ActivityEntry[]>([]);
  const [registry, setRegistry] = useState<Registry | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);

  const reload = useCallback(async () => {
    const [list, names] = await Promise.all([api.listActivity(), api.listRegistry()]);
    setEntries(list);
    setRegistry(names);
  }, []);

  useEffect(() => {
    reload().catch((failure: unknown) => setError(asFailure(failure).message));
  }, [reload]);

  return (
    <main className="page">
      <div className="page-column">
        <section>
          <div className="row between">
            <h2>Activity</h2>
            {!confirming && (
              <button className="secondary small" type="button" disabled={entries.length === 0} onClick={() => setConfirming(true)}>
                Clear activity log
              </button>
            )}
          </div>
          <p className="hint">This list is kept on this computer for your account. It records what was asked and decided, never the contents.</p>
          {confirming && (
            <div className="delete-form">
              <p>Delete every entry in this list? It can&rsquo;t be undone.</p>
              <div className="row">
                <button
                  className="danger small"
                  type="button"
                  onClick={() => {
                    setConfirming(false);
                    setError(null);
                    api
                      .clearActivity()
                      .then(async (removed) => {
                        await reload();
                        setNotice(`Deleted ${removed} ${removed === 1 ? "entry" : "entries"}.`);
                      })
                      .catch((failure: unknown) => setError(asFailure(failure).message));
                  }}
                >
                  Delete entries
                </button>
                <button className="secondary small" type="button" onClick={() => setConfirming(false)}>
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
          {entries.length === 0 ? (
            <p className="muted">Nothing yet. Reading a project, opening a program or setting a reminder will be listed here.</p>
          ) : (
            <ul className="list">
              {entries.map((entry) => (
                <li key={entry.id}>
                  <strong>{toolLabel(entry.tool)}</strong>
                  {registry && entry.resource_id && <span className="tag">{resourceName(registry, entry.resource_id)}</span>}
                  <div>{activityLine(entry)}</div>
                  <div className="hint">{formatWhen(entry.at)}</div>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>
    </main>
  );
}
