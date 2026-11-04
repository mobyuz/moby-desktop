import { useState } from "react";
import { EXPRESSION_META } from "./face/expressions";
import { MobyFace } from "./face/MobyFace";
import { Icon } from "./icons";
import { Account } from "./screens/Account";
import { Auth } from "./screens/Auth";
import { Chat } from "./screens/Chat";
import { AppProvider, useApp } from "./store";

function Shell() {
  const app = useApp();
  const [accountOpen, setAccountOpen] = useState(false);

  if (app.status === "starting") {
    return (
      <main className="center">
        <MobyFace expression="thinking" />
      </main>
    );
  }

  if (app.status === "offline") {
    return (
      <main className="center">
        <MobyFace expression="disconnected" />
        <h1>Can't connect to Moby</h1>
        <p className="muted">Check your connection and try again.</p>
        <button className="primary" type="button" onClick={app.retryStart}>
          Try again
        </button>
      </main>
    );
  }

  if (!app.profile) return <Auth />;

  const name = app.profile.display_name ?? app.profile.email;

  return (
    <div className="app" data-expression={app.expression}>
      <header className="bar">
        {accountOpen ? (
          <button className="ghost" type="button" onClick={() => setAccountOpen(false)}>
            <Icon name="back" />
            Back to chat
          </button>
        ) : (
          <>
            <div className="bar-title">
              <strong>Moby</strong>
              <span className="muted" role="status">
                {EXPRESSION_META[app.expression].status}
              </span>
            </div>
            <button className="avatar" type="button" title="Account" aria-label="Account" onClick={() => setAccountOpen(true)}>
              {name.slice(0, 1).toUpperCase()}
            </button>
          </>
        )}
      </header>
      {accountOpen ? <Account /> : <Chat />}
    </div>
  );
}

export function App() {
  return (
    <AppProvider>
      <Shell />
    </AppProvider>
  );
}
