import { useState, type FormEvent } from "react";
import { MobyFace } from "../face/MobyFace";
import { DEMO_ACCOUNT, useApp } from "../store";

export function Auth() {
  const app = useApp();
  const [mode, setMode] = useState<"login" | "register">("login");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [accepted, setAccepted] = useState(false);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    void (mode === "login" ? app.signIn(email, password) : app.register(email, password, accepted));
  };

  return (
    <main className="auth">
      <div className="auth-art">
        <MobyFace expression={app.authBusy ? "thinking" : app.authError ? "confused" : "idle"} />
        <h1>Moby</h1>
        <p className="muted">Your desk companion, on your computer.</p>
      </div>

      <form className="card auth-form" onSubmit={submit} aria-labelledby="auth-title">
        <div className="tabs" role="tablist" aria-label="Account">
          <button type="button" role="tab" aria-selected={mode === "login"} onClick={() => setMode("login")}>
            Sign in
          </button>
          <button type="button" role="tab" aria-selected={mode === "register"} onClick={() => setMode("register")}>
            Create account
          </button>
        </div>
        <h2 id="auth-title">{mode === "login" ? "Welcome back" : "Create your account"}</h2>

        <label>
          Email
          <input type="email" autoComplete="email" value={email} onChange={(e) => setEmail(e.target.value)} required />
        </label>
        <label>
          Password
          <input
            type="password"
            autoComplete={mode === "login" ? "current-password" : "new-password"}
            value={password}
            minLength={15}
            onChange={(e) => setPassword(e.target.value)}
            required
          />
          <span className="hint">At least 15 characters.</span>
        </label>

        {mode === "register" && (
          <label className="check">
            <input type="checkbox" checked={accepted} onChange={(e) => setAccepted(e.target.checked)} />
            <span>I accept the terms and the privacy policy.</span>
          </label>
        )}

        {app.authError && (
          <p className="error" role="alert">
            {app.authError}
          </p>
        )}

        <button className="primary" type="submit" disabled={app.authBusy}>
          {app.authBusy ? "Please wait…" : mode === "login" ? "Sign in" : "Create account"}
        </button>
        <button className="secondary" type="button" disabled title="Opens your browser. Available when the backend is connected.">
          Continue with Google
        </button>

        <div className="divider">or</div>
        <button
          className="secondary"
          type="button"
          onClick={() => {
            setEmail(DEMO_ACCOUNT.email);
            setPassword(DEMO_ACCOUNT.password);
            setMode("login");
          }}
        >
          Fill in the demo account
        </button>
        <button className="link" type="button" onClick={app.continueOffline}>
          Continue without an account
        </button>
        <p className="hint">Demo mode: no server is contacted and nothing leaves this computer.</p>
      </form>
    </main>
  );
}
