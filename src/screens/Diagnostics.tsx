import { PROTOCOL_VERSION } from "@mobyuz/moby-protocol";
import pkg from "../../package.json";
import { useApp } from "../store";

export function Diagnostics() {
  const app = useApp();
  const robot = app.devices[0];

  return (
    <div className="stack">
      <section className="card">
        <h2>Versions</h2>
        <dl className="facts">
          <div>
            <dt>App</dt>
            <dd>{pkg.version}</dd>
          </div>
          <div>
            <dt>Protocol</dt>
            <dd>{PROTOCOL_VERSION}</dd>
          </div>
          <div>
            <dt>Robot firmware</dt>
            <dd>{robot?.firmware ?? "none"}</dd>
          </div>
          <div>
            <dt>Mode</dt>
            <dd>Demo (no backend)</dd>
          </div>
        </dl>
      </section>

      <section className="card">
        <h2>Link quality</h2>
        <dl className="facts">
          <div>
            <dt>Dropped audio frames</dt>
            <dd>0</dd>
          </div>
          <div>
            <dt>Heartbeat</dt>
            <dd>{robot?.connected ? "healthy" : "lost"}</dd>
          </div>
          <div>
            <dt>Backend</dt>
            <dd>Not connected (demo)</dd>
          </div>
        </dl>
      </section>

      <section className="card">
        <h2>Log</h2>
        <p className="hint">Sanitized: no messages, voice, or passwords are written here.</p>
        <ol className="log" aria-label="Log lines">
          {[...app.log].reverse().map((line, index) => (
            <li key={`${line.at}-${index}`} data-level={line.level}>
              <time>{new Date(line.at).toLocaleTimeString("en")}</time> {line.text}
            </li>
          ))}
        </ol>
      </section>
    </div>
  );
}
