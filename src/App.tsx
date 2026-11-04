import { useState } from "react";
import { Account } from "./screens/Account";
import { Approvals } from "./screens/Approvals";
import { Auth } from "./screens/Auth";
import { Devices } from "./screens/Devices";
import { Diagnostics } from "./screens/Diagnostics";
import { Home } from "./screens/Home";
import { Privacy } from "./screens/Privacy";
import { Settings } from "./screens/Settings";
import { Tools } from "./screens/Tools";
import { AppProvider, useApp } from "./store";

type Screen = "home" | "devices" | "tools" | "approvals" | "account" | "privacy" | "settings" | "diagnostics";

const NAV: { id: Screen; label: string }[] = [
  { id: "home", label: "Home" },
  { id: "devices", label: "Devices" },
  { id: "tools", label: "Tools & projects" },
  { id: "approvals", label: "Approvals" },
  { id: "account", label: "Account & usage" },
  { id: "privacy", label: "Memory & privacy" },
  { id: "settings", label: "Settings" },
  { id: "diagnostics", label: "Diagnostics" },
];

function Shell() {
  const app = useApp();
  const [screen, setScreen] = useState<Screen>("home");
  const pending = app.approvals.filter((item) => item.status === "pending").length;

  if (!app.profile && !app.offline) return <Auth />;

  const title = NAV.find((item) => item.id === screen)?.label ?? "";

  return (
    <div className="shell">
      <nav className="sidebar" aria-label="Main">
        <div className="brand">Moby</div>
        <ul>
          {NAV.map((item) => (
            <li key={item.id}>
              <button type="button" aria-current={screen === item.id ? "page" : undefined} onClick={() => setScreen(item.id)}>
                {item.label}
                {item.id === "approvals" && pending > 0 && <span className="badge">{pending}</span>}
              </button>
            </li>
          ))}
        </ul>
        <div className="who">
          {app.profile ? (
            <>
              <span className="muted">Signed in as</span>
              <strong>{app.profile.display_name ?? app.profile.email}</strong>
            </>
          ) : (
            <>
              <span className="muted">No account</span>
              <button className="link" type="button" onClick={app.leaveOffline}>
                Sign in
              </button>
            </>
          )}
        </div>
      </nav>

      <div className="content">
        <header className="topbar">
          <h1>{title}</h1>
          <span className="mode">Demo mode · no server, no robot needed</span>
        </header>
        <main>
          {screen === "home" && <Home go={setScreen} />}
          {screen === "devices" && <Devices />}
          {screen === "tools" && <Tools />}
          {screen === "approvals" && <Approvals />}
          {screen === "account" && <Account onSignIn={app.leaveOffline} />}
          {screen === "privacy" && <Privacy />}
          {screen === "settings" && <Settings />}
          {screen === "diagnostics" && <Diagnostics />}
        </main>
      </div>
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
