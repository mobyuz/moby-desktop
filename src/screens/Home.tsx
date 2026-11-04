import { EXPRESSIONS } from "@mobyuz/moby-protocol";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { EXPRESSION_META } from "../face/expressions";
import { MobyFace } from "../face/MobyFace";
import { useApp } from "../store";

const SUGGESTIONS = ["What is the git status?", "Open my editor", "Remind me about stand-up", "Hello Moby"];

export function Home({ go }: { go: (screen: "approvals") => void }) {  const app = useApp();
  const [text, setText] = useState("");
  const endRef = useRef<HTMLDivElement>(null);
  const meta = EXPRESSION_META[app.expression];
  const robot = app.devices[0];
  const cloudOff = !app.profile;
  const pending = app.approvals.filter((item) => item.status === "pending");

  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [app.messages.length]);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (cloudOff) return;
    void app.send(text);
    setText("");
  };

  return (
    <div className="home-grid">
      <section className="card stage" aria-label="Moby">
        <MobyFace expression={app.expression} />
        <p className="state">{meta.label}</p>
        <p className="muted">{meta.status}</p>
        <ul className="indicators">
          <li data-on={meta.greenLed !== "off"}>{meta.greenLed !== "off" ? "Microphone path open" : "Microphone path off"}</li>
          <li data-on={meta.redLed !== "off"}>{meta.redLed !== "off" ? "Audio session active" : "No audio session"}</li>
          <li data-on={Boolean(robot?.connected)}>{robot ? (robot.connected ? "Robot connected" : "Robot disconnected") : "No robot paired"}</li>
        </ul>

        <details className="preview">
          <summary>Preview expressions</summary>
          <div className="chips" role="group" aria-label="Expressions">
            <button type="button" aria-pressed={app.preview === null} onClick={() => app.setPreview(null)}>
              Live
            </button>
            {EXPRESSIONS.map((name) => (
              <button key={name} type="button" aria-pressed={app.preview === name} onClick={() => app.setPreview(name)}>
                {EXPRESSION_META[name].label}
              </button>
            ))}
          </div>
        </details>
      </section>

      <section className="card chat" aria-label="Conversation">
        <div className="messages" role="log" aria-live="polite">
          {app.messages.map((message) => (
            <p key={message.id} className={`bubble ${message.role}`}>
              {message.text}
            </p>
          ))}
          {pending.map((item) => (
            <div key={item.id} className="bubble ask">
              <strong>{item.title}</strong>
              <span className="muted">{item.detail}</span>
              <span className="row">
                <button className="primary small" type="button" onClick={() => app.resolveApproval(item.id, true)}>
                  Approve
                </button>
                <button className="secondary small" type="button" onClick={() => app.resolveApproval(item.id, false)}>
                  Deny
                </button>
                <button className="link" type="button" onClick={() => go("approvals")}>
                  Details
                </button>
              </span>
            </div>
          ))}
          <div ref={endRef} />
        </div>

        {cloudOff && <p className="notice">Sign in to chat with Moby. The virtual face, tools and settings work without an account.</p>}

        <div className="chips" aria-label="Suggestions">
          {SUGGESTIONS.map((suggestion) => (
            <button key={suggestion} type="button" disabled={cloudOff || app.busy} onClick={() => void app.send(suggestion)}>
              {suggestion}
            </button>
          ))}
        </div>

        <form className="composer" onSubmit={submit}>
          <input
            aria-label="Message to Moby"
            placeholder={cloudOff ? "Sign in to send a message" : "Type a message (typing is the fallback when the robot mic is off)"}
            value={text}
            disabled={cloudOff}
            onChange={(e) => setText(e.target.value)}
          />
          <button className="primary" type="submit" disabled={cloudOff || app.busy || text.trim() === ""}>
            Send
          </button>
        </form>
      </section>
    </div>
  );
}
