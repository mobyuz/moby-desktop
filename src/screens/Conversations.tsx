import { useState } from "react";
import { asFailure } from "../api";
import { formatWhen } from "../history";
import { useApp } from "../store";

export function Conversations({ onDone }: { onDone: () => void }) {
  const app = useApp();
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);

  const attempt = (action: Promise<unknown>, after?: () => void) => {
    setError(null);
    action.then(after).catch((failure: unknown) => setError(asFailure(failure).message));
  };

  return (
    <main className="page">
      <div className="page-column">
        <section>
          <div className="row between">
            <h2>Conversations</h2>
            <button
              className="primary small"
              type="button"
              disabled={app.sending}
              onClick={() => {
                app.newConversation();
                onDone();
              }}
            >
              New conversation
            </button>
          </div>
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          {app.conversations.length === 0 ? (
            <p className="muted">Nothing here yet. Your conversations with Moby will be listed here.</p>
          ) : (
            <ul className="conversations">
              {app.conversations.map((item) => (
                <li key={item.id} className="conversation">
                  {confirming === item.id ? (
                    <>
                      <span className="conversation-ask">Delete this conversation?</span>
                      <div className="row">
                        <button
                          className="danger small"
                          type="button"
                          onClick={() => {
                            setConfirming(null);
                            attempt(app.deleteConversation(item.id));
                          }}
                        >
                          Delete
                        </button>
                        <button className="secondary small" type="button" onClick={() => setConfirming(null)}>
                          Cancel
                        </button>
                      </div>
                    </>
                  ) : (
                    <>
                      <button
                        className="conversation-open"
                        type="button"
                        aria-current={item.id === app.activeId ? "true" : undefined}
                        disabled={app.sending}
                        onClick={() => attempt(app.openConversation(item.id), onDone)}
                      >
                        <strong>{item.title}</strong>
                        <span className="hint">{formatWhen(item.updated_at)}</span>
                      </button>
                      <button className="secondary small" type="button" aria-label={`Delete ${item.title}`} disabled={app.sending} onClick={() => setConfirming(item.id)}>
                        Delete
                      </button>
                    </>
                  )}
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>
    </main>
  );
}
