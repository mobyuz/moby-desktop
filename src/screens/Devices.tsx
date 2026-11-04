import { PROTOCOL_VERSION } from "@mobyuz/moby-protocol";
import { useEffect, useState } from "react";
import { useApp } from "../store";

export function Devices() {
  const app = useApp();
  const [beats, setBeats] = useState(0);
  const [pairing, setPairing] = useState(false);
  const [secondsLeft, setSecondsLeft] = useState(30);

  useEffect(() => {
    const timer = window.setInterval(() => setBeats((count) => count + 1), 1000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!pairing) return;
    setSecondsLeft(30);
    const timer = window.setInterval(() => setSecondsLeft((left) => Math.max(0, left - 1)), 1000);
    return () => window.clearInterval(timer);
  }, [pairing]);

  const code = "7F3A-90C2";

  return (
    <div className="stack">
      <section className="card">
        <div className="row between">
          <h2>Robots</h2>
          <button className="primary small" type="button" onClick={() => setPairing(true)} disabled={pairing}>
            Pair a robot
          </button>
        </div>
        <p className="hint">
          You have no physical robot yet, so these are simulated robots that speak the same USB protocol (version {PROTOCOL_VERSION}).
        </p>

        {pairing && (
          <div className="pairing" role="group" aria-label="Pairing">
            <strong>New simulated robot found</strong>
            <p className="muted">
              Check that the robot shows the code <code>{code}</code>, then press its talk button within {secondsLeft} seconds.
            </p>
            <div className="row">
              <button
                className="primary small"
                type="button"
                disabled={secondsLeft === 0}
                onClick={() => {
                  app.addDevice(`Moby Simulator ${app.devices.length + 1}`);
                  setPairing(false);
                }}
              >
                I pressed the talk button
              </button>
              <button className="secondary small" type="button" onClick={() => setPairing(false)}>
                Cancel
              </button>
            </div>
          </div>
        )}

        {app.devices.length === 0 && <p className="muted">No robots paired. The virtual Moby still works.</p>}
        <ul className="list">
          {app.devices.map((device) => (
            <li key={device.id} className="device">
              <div className="row between">
                <span>
                  <strong>{device.name}</strong>
                  <span className={`tag ${device.connected ? "good" : "bad"}`}>{device.connected ? "Connected" : "Disconnected"}</span>
                  <span className="muted block">
                    {device.id} · firmware {device.firmware} · protocol {device.protocol}
                  </span>
                </span>
                <button className="secondary small" type="button" onClick={() => app.removeDevice(device.id)}>
                  Unpair
                </button>
              </div>

              <dl className="facts">
                <div>
                  <dt>Heartbeat</dt>
                  <dd>{device.connected ? `every 1 s, ${beats} received` : "lost after 3 missed"}</dd>
                </div>
                <div>
                  <dt>Microphone switch</dt>
                  <dd>{device.hardwareMuted ? "Off (hardware mute)" : "On"}</dd>
                </div>
                <div>
                  <dt>Paired</dt>
                  <dd>{new Date(device.pairedAt).toLocaleString("en", { dateStyle: "medium", timeStyle: "short" })}</dd>
                </div>
              </dl>

              <div className="row">
                <button className="secondary small" type="button" aria-pressed={device.hardwareMuted} onClick={() => app.toggleHardwareMute(device.id)}>
                  {device.hardwareMuted ? "Flip switch: mic on" : "Flip switch: mic off"}
                </button>
                <button className="secondary small" type="button" onClick={() => app.toggleConnection(device.id)}>
                  {device.connected ? "Simulate unplug" : "Simulate reconnect"}
                </button>
              </div>
            </li>
          ))}
        </ul>
        <p className="hint">The hardware mute switch always wins: when it is off, Moby cannot listen, whatever the app says.</p>
      </section>
    </div>
  );
}
