import { useEffect, useState, type ReactNode } from "react";
import { EXPRESSION_META } from "./face/expressions";
import { MobyFace } from "./face/MobyFace";
import { Icon } from "./icons";
import { Account } from "./screens/Account";
import { Auth } from "./screens/Auth";
import { Chat } from "./screens/Chat";
import { Conversations } from "./screens/Conversations";
import { Settings } from "./screens/Settings";
import { AppProvider, useApp } from "./store";

type View = "chat" | "conversations" | "account" | "settings";

function Shell() {
  const app = useApp();
  const [view, setView] = useState<View>("chat");
  const signedIn = app.profile !== null;

  // After signing out, only Settings stays reachable. The next sign-in starts at the chat.
  useEffect(() => {
    if (!signedIn) setView((current) => (current === "settings" ? current : "chat"));
  }, [signedIn]);

  useEffect(() => {
    if (view === "chat") return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setView("chat");
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [view]);

  const settingsLink = (
    <button className="ghost corner" type="button" onClick={() => setView("settings")}>
      <Icon name="settings" />
      Settings
    </button>
  );

  if (app.status === "starting") {
    return (
      <main className="center">
        <MobyFace expression="thinking" />
      </main>
    );
  }

  if (view !== "chat" && (view === "settings" || signedIn)) {
    const page: Record<Exclude<View, "chat">, ReactNode> = {
      conversations: <Conversations onDone={() => setView("chat")} />,
      account: <Account />,
      settings: <Settings />,
    };
    return (
      <div className="app">
        <header className="bar">
          <button className="ghost" type="button" onClick={() => setView("chat")}>
            <Icon name="back" />
            {signedIn ? "Back to chat" : "Back"}
          </button>
        </header>
        {app.storeWarning && (
          <p className="notice banner" role="status">
            {app.storeWarning}
          </p>
        )}
        {page[view]}
      </div>
    );
  }

  if (app.status === "offline") {
    return (
      <>
        <main className="center">
          <MobyFace expression="disconnected" />
          <h1>Can't connect to Moby</h1>
          <p className="muted">Check your connection and try again.</p>
          <button className="primary" type="button" onClick={app.retryStart}>
            Try again
          </button>
        </main>
        {settingsLink}
      </>
    );
  }

  if (!app.profile) {
    return (
      <>
        <Auth />
        {settingsLink}
      </>
    );
  }

  const name = app.profile.display_name ?? app.profile.email;

  return (
    <div className="app" data-expression={app.expression}>
      <header className="bar">
        <div className="bar-title">
          <strong>Moby</strong>
          <span className="muted" role="status">
            {EXPRESSION_META[app.expression].status}
          </span>
        </div>
        <div className="bar-actions">
          <button className="icon-button" type="button" title="New conversation" aria-label="New conversation" disabled={app.sending} onClick={app.newConversation}>
            <Icon name="plus" />
          </button>
          <button className="icon-button" type="button" title="Conversations" aria-label="Conversations" onClick={() => setView("conversations")}>
            <Icon name="chats" />
          </button>
          <button className="icon-button" type="button" title="Settings" aria-label="Settings" onClick={() => setView("settings")}>
            <Icon name="settings" />
          </button>
          <button className="avatar" type="button" title="Account" aria-label="Account" onClick={() => setView("account")}>
            {name.slice(0, 1).toUpperCase()}
          </button>
        </div>
      </header>
      {app.storeWarning && (
        <p className="notice banner" role="status">
          {app.storeWarning}
        </p>
      )}
      <Chat />
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
