import { useState, type FormEvent } from "react";
import { api, asFailure } from "../api";
import { MobyFace } from "../face/MobyFace";
import { Icon } from "../icons";
import { useApp } from "../store";

type Mode = "signIn" | "register" | "forgot";

const TITLES: Record<Mode, string> = {
  signIn: "Sign in to Moby",
  register: "Create your account",
  forgot: "Reset your password",
};

const ACTIONS: Record<Mode, string> = {
  signIn: "Sign in",
  register: "Create account",
  forgot: "Send reset link",
};

export function Auth() {
  const app = useApp();
  const [mode, setMode] = useState<Mode>("signIn");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [reveal, setReveal] = useState(false);
  const [accepted, setAccepted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const switchTo = (next: Mode) => {
    setMode(next);
    setError(null);
    setNotice(null);
    setPassword("");
  };

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (mode === "register" && !accepted) {
      setError("Accept the terms and privacy policy to continue.");
      return;
    }
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      if (mode === "signIn") {
        await app.signIn(email, password);
      } else if (mode === "register") {
        await api.register(email, password);
        switchTo("signIn");
        setNotice("Check your email to confirm your account, then sign in.");
      } else {
        await api.forgotPassword(email);
        switchTo("signIn");
        setNotice("If this email has an account, a reset link is on its way.");
      }
    } catch (failure) {
      setError(asFailure(failure).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="center" data-expression={busy ? "thinking" : error ? "confused" : "idle"}>
      <MobyFace expression={busy ? "thinking" : error ? "confused" : "idle"} />
      <form className="auth-form" onSubmit={(event) => void submit(event)} aria-labelledby="auth-title">
        <h1 id="auth-title">{TITLES[mode]}</h1>
        {notice && (
          <p className="notice" role="status">
            {notice}
          </p>
        )}

        <label>
          Email
          <input type="email" autoComplete="email" value={email} onChange={(e) => setEmail(e.target.value)} required />
        </label>

        {mode !== "forgot" && (
          <div className="field">
            <label htmlFor="auth-password">Password</label>
            <div className="input-action">
              <input
                id="auth-password"
                type={reveal ? "text" : "password"}
                autoComplete={mode === "signIn" ? "current-password" : "new-password"}
                value={password}
                minLength={15}
                maxLength={128}
                onChange={(e) => setPassword(e.target.value)}
                required
              />
              <button type="button" aria-label={reveal ? "Hide password" : "Show password"} title={reveal ? "Hide password" : "Show password"} onClick={() => setReveal((shown) => !shown)}>
                <Icon name={reveal ? "eyeOff" : "eye"} />
              </button>
            </div>
            {mode === "register" && <span className="hint">At least 15 characters.</span>}
          </div>
        )}

        {mode === "register" && (
          <label className="check">
            <input type="checkbox" checked={accepted} onChange={(e) => setAccepted(e.target.checked)} />
            <span>I accept the terms and the privacy policy.</span>
          </label>
        )}

        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}

        <button className="primary" type="submit" disabled={busy}>
          {busy ? "Please wait…" : ACTIONS[mode]}
        </button>

        <div className="auth-links">
          {mode === "signIn" ? (
            <>
              <button className="link" type="button" onClick={() => switchTo("forgot")}>
                Forgot password?
              </button>
              <button className="link" type="button" onClick={() => switchTo("register")}>
                Create account
              </button>
            </>
          ) : (
            <button className="link" type="button" onClick={() => switchTo("signIn")}>
              Back to sign in
            </button>
          )}
        </div>
      </form>
    </main>
  );
}
