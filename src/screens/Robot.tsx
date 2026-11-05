import { useCallback, useEffect, useRef, useState } from "react";
import { api, asFailure } from "../api";
import { MobyFace } from "../face/MobyFace";
import { droppedFrames, isLive, PAIR_SECONDS, robotDetail, robotFace, robotStatus, STATUS_LABEL } from "../robot";
import { useApp } from "../store";
import type { PairedRobot, RobotPort } from "../types";
import { RobotDevPanel } from "./RobotDev";

/** Counts down from the pairing window, starting when the robot starts waiting for its button. */
function useCountdown(running: boolean, seconds: number): number {
  const [left, setLeft] = useState(seconds);
  useEffect(() => {
    if (!running) return;
    setLeft(seconds);
    const started = Date.now();
    const timer = window.setInterval(() => setLeft(Math.max(0, seconds - Math.floor((Date.now() - started) / 1000))), 500);
    return () => window.clearInterval(timer);
  }, [running, seconds]);
  return left;
}

/** The USB link to Moby: its state, pairing, and the robots paired with this computer. */
export function Robot() {
  const app = useApp();
  const robot = app.robot;
  const status = robotStatus(robot);
  const live = isLive(robot);
  const [ports, setPorts] = useState<RobotPort[] | null>(null);
  const [chosen, setChosen] = useState("");
  const [devices, setDevices] = useState<PairedRobot[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [confirmingPair, setConfirmingPair] = useState(false);
  const [removing, setRemoving] = useState<string | null>(null);
  const failureRef = useRef<HTMLParagraphElement>(null);
  const waiting = robot.pair_step === "press_button";
  const secondsLeft = useCountdown(waiting, PAIR_SECONDS);

  const fail = (failure: unknown) => setError(asFailure(failure).message);

  const reloadDevices = useCallback(() => {
    api
      .robotDevices()
      .then(setDevices)
      .catch((failure: unknown) => setError(asFailure(failure).message));
  }, []);

  // The list of paired robots changes when pairing finishes, a pairing is removed, or the robot is reset.
  const pairedHere = robot.device?.paired_here ?? null;
  useEffect(reloadDevices, [reloadDevices, pairedHere]);

  const loadPorts = useCallback(() => {
    api
      .robotPorts()
      .then((list) => {
        setPorts(list);
        setChosen((current) => (list.some((port) => port.name === current) ? current : (list[0]?.name ?? "")));
      })
      .catch((failure: unknown) => setError(asFailure(failure).message));
  }, []);

  useEffect(() => {
    if (robot.pair_failure) failureRef.current?.focus();
  }, [robot.pair_failure]);

  const startPairing = () => {
    setConfirmingPair(false);
    setError(null);
    api.robotBeginPairing().catch(fail);
  };

  return (
    <main className="page">
      <div className="page-column">
        <section aria-labelledby="robot-title">
          <h2 id="robot-title">Moby robot</h2>
          <div className="robot-card">
            <MobyFace expression={robotFace(robot) ?? "disconnected"} />
            <div className="grow">
              <div className="row">
                <span className="status-chip" data-status={status}>
                  {STATUS_LABEL[status]}
                </span>
                {robot.simulated && <span className="tag">Simulator</span>}
              </div>
              <p role="status" className="robot-detail">
                {robotDetail(robot)}
              </p>
              {robot.muted && live && <p className="hint">The mute switch on Moby decides whether it can hear. This app can only show it, never change it.</p>}
              {robot.device && live && (
                <p className="hint">
                  {robot.device.name} · firmware {robot.device.fw_version} · board {robot.device.hw_rev}
                  {robot.port ? ` · ${robot.port}` : ""}
                </p>
              )}
              {robot.device && live && !robot.device.audio && <p className="hint">This firmware offers no voice link yet. Pairing and status work.</p>}
              {robot.error && (
                <p className="error" role="alert">
                  Moby reported {robot.error.code}: {robot.error.detail}
                </p>
              )}
            </div>
          </div>
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          <div className="row">
            {robot.phase === "disconnected" ? (
              <button className="primary" type="button" onClick={() => api.robotConnect().catch(fail)}>
                Connect to Moby
              </button>
            ) : (
              <button className="secondary" type="button" onClick={() => api.robotDisconnect().catch(fail)}>
                Disconnect
              </button>
            )}
          </div>
          <details
            className="details-box"
            onToggle={(event) => {
              if (event.currentTarget.open) loadPorts();
            }}
          >
            <summary>Choose a port yourself</summary>
            <p className="hint">Moby shows up as a USB serial port. Only USB serial ports that Windows lists can be used.</p>
            {ports !== null && ports.length === 0 && <p className="muted">No USB serial ports found.</p>}
            {ports !== null && ports.length > 0 && (
              <div className="row">
                <label className="grow">
                  Port
                  <select value={chosen} onChange={(event) => setChosen(event.target.value)}>
                    {ports.map((port) => (
                      <option key={port.name} value={port.name}>
                        {port.name} · {port.label}
                        {port.kind === "moby" ? " (looks like Moby)" : ""}
                      </option>
                    ))}
                  </select>
                </label>
                <button className="secondary small" type="button" disabled={chosen === ""} onClick={() => api.robotConnect(chosen).catch(fail)}>
                  Connect to this port
                </button>
                <button className="secondary small" type="button" onClick={loadPorts}>
                  Refresh
                </button>
              </div>
            )}
          </details>
        </section>

        {live && robot.device && (
          <section aria-labelledby="pair-title">
            <h2 id="pair-title">Pairing</h2>
            {robot.pair_failure && (
              <p ref={failureRef} className="error" role="alert" tabIndex={-1}>
                {robot.pair_failure.message}
              </p>
            )}
            {robot.phase === "pairing" ? (
              <div className="pair-box" role="status">
                {robot.pair_step === "starting" && <p>Contacting Moby…</p>}
                {waiting && (
                  <>
                    <p className="pair-prompt">Press the talk button on Moby</p>
                    <p className="hint">
                      Within {secondsLeft} {secondsLeft === 1 ? "second" : "seconds"}. This proves that you are next to the robot.
                    </p>
                  </>
                )}
                {robot.pair_step === "saving" && <p>Saving the pairing…</p>}
                <div className="row">
                  <button className="secondary small" type="button" onClick={() => api.robotCancelPairing().catch(fail)}>
                    Cancel
                  </button>
                </div>
              </div>
            ) : robot.phase === "ready" ? (
              <p>
                <span className="ok">Paired.</span> Moby will reconnect to this computer on its own.
              </p>
            ) : robot.phase === "unpaired" ? (
              confirmingPair ? (
                <div className="delete-form">
                  <p>
                    Pair <strong>{robot.device.name}</strong> with this computer? Moby will then only answer this computer. When you continue, press the talk button on Moby within {PAIR_SECONDS} seconds.
                  </p>
                  <div className="row">
                    <button className="primary small" type="button" onClick={startPairing}>
                      Start pairing
                    </button>
                    <button className="secondary small" type="button" onClick={() => setConfirmingPair(false)}>
                      Cancel
                    </button>
                  </div>
                </div>
              ) : (
                <div className="row">
                  <button className="primary" type="button" disabled={robot.muted} onClick={() => setConfirmingPair(true)}>
                    Pair with this Moby
                  </button>
                  {robot.muted && <span className="hint">Turn the mute switch off first.</span>}
                </div>
              )
            ) : (
              <p className="muted">Pairing is not available while the link is in this state.</p>
            )}
          </section>
        )}

        <section aria-labelledby="paired-title">
          <h2 id="paired-title">Paired with this computer</h2>
          <p className="hint">This list belongs to your Windows user, not to a Moby account. Signing out does not change it.</p>
          {devices.length === 0 ? (
            <p className="muted">No robots yet.</p>
          ) : (
            <ul className="list">
              {devices.map((device) => (
                <li key={device.id}>
                  <div className="row between">
                    <div className="grow">
                      <strong>{device.name}</strong>
                      {robot.device?.id === device.id && live && <span className="tag">Connected</span>}
                      <div className="hint path">{device.id}</div>
                    </div>
                    {removing === device.id ? (
                      <div className="row">
                        <button
                          className="danger small"
                          type="button"
                          onClick={() => {
                            setRemoving(null);
                            api
                              .robotUnpair(device.id)
                              .then(reloadDevices)
                              .catch(fail);
                          }}
                        >
                          Remove
                        </button>
                        <button className="secondary small" type="button" onClick={() => setRemoving(null)}>
                          Cancel
                        </button>
                      </div>
                    ) : (
                      <button className="secondary small" type="button" aria-label={`Unpair ${device.name}`} onClick={() => setRemoving(device.id)}>
                        Unpair
                      </button>
                    )}
                  </div>
                  {removing === device.id && (
                    <p className="hint">
                      This removes the saved key from this computer. Moby keeps its own copy until you hold its talk button for 10 seconds, so reset it too if you want to pair it again.
                    </p>
                  )}
                </li>
              ))}
            </ul>
          )}
        </section>

        <section aria-labelledby="link-title">
          <h2 id="link-title">Link details</h2>
          <p className="hint">Counts only. {droppedFrames(robot)} damaged or unusable messages from the robot were dropped.</p>
          <dl className="stats">
            <dt>Messages accepted</dt>
            <dd>{robot.stats.frames_ok}</dd>
            <dt>Damaged frames</dt>
            <dd>{robot.stats.bad_crc + robot.stats.bad_frame + robot.stats.bad_frame_version}</dd>
            <dt>Oversized frames</dt>
            <dd>{robot.stats.oversized}</dd>
            <dt>Unreadable messages</dt>
            <dd>{robot.stats.malformed_json + robot.stats.wrong_sender}</dd>
            <dt>Audio ignored (not supported yet)</dt>
            <dd>{robot.stats.audio_frames_ignored + robot.stats.audio_events_ignored}</dd>
          </dl>
        </section>

        {import.meta.env.DEV && <RobotDevPanel />}
      </div>
    </main>
  );
}
