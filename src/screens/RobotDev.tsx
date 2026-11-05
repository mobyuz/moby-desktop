import { useState } from "react";
import { api, asFailure } from "../api";
import { useApp } from "../store";
import type { DevFault, RobotDevAction } from "../types";

const FAULTS: [string, DevFault][] = [
  ["Lose a frame", "bad_sequence"],
  ["Repeat a frame", "duplicate"],
  ["Swap two frames", "swap"],
  ["Wrong frame size", "wrong_size"],
  ["Overrun", "overrun"],
  ["Cut off", "cut_off"],
  ["Never end", "never_ends"],
  ["Wrong frame count", "wrong_count"],
  ["Wrong stream", "wrong_stream"],
  ["Mute mid-stream", "mute_mid_stream"],
  ["Mute, keep sending", "mute_and_keep_sending"],
  ["Robot cancels", "robot_cancel"],
  ["No fault", "none"],
];

/**
 * Drives the simulated robot. This file is only reached when `import.meta.env.DEV` is true, so the
 * production bundle drops it, and the Rust command it calls does not exist in a release build.
 */
export function RobotDevPanel() {
  const [error, setError] = useState<string | null>(null);
  const [sent, setSent] = useState<string | null>(null);

  const send = (label: string, action: RobotDevAction) => {
    setError(null);
    api
      .robotDev(action)
      .then(() => setSent(label))
      .catch((failure: unknown) => setError(asFailure(failure).message));
  };

  const button = (label: string, action: RobotDevAction) => (
    <button key={label} className="secondary small" type="button" onClick={() => send(label, action)}>
      {label}
    </button>
  );

  return (
    <section aria-labelledby="sim-title" className="dev-panel">
      <h2 id="sim-title">Simulator (development builds only)</h2>
      <p className="hint">A robot in software that speaks the real protocol. It is not in release builds and proves nothing about real USB.</p>
      <div className="row">
        {button("Plug in a new robot", { action: "attach", fresh: true, paired_elsewhere: false, incompatible: false })}
        {button("Plug in one paired elsewhere", { action: "attach", fresh: true, paired_elsewhere: true, incompatible: false })}
        {button("Plug in an incompatible one", { action: "attach", fresh: true, paired_elsewhere: false, incompatible: true })}
        {button("Reconnect the same robot", { action: "attach", fresh: false, paired_elsewhere: false, incompatible: false })}
        {button("Remove", { action: "detach" })}
      </div>
      <h3>Robot controls</h3>
      <div className="row">
        {button("Press talk", { action: "press" })}
        {button("Release talk", { action: "release" })}
        {button("Hold talk 10 s (reset pairing)", { action: "long_press" })}
        {button("Mute on", { action: "mute", muted: true })}
        {button("Mute off", { action: "mute", muted: false })}
      </div>
      <div className="row">
        {button("Listening", { action: "show", expression: "listening" })}
        {button("Thinking", { action: "show", expression: "thinking" })}
        {button("Speaking", { action: "show", expression: "speaking" })}
        {button("Idle", { action: "show", expression: "idle" })}
      </div>
      <h3>Next voice stream goes wrong</h3>
      <p className="hint">Pair the simulated robot first, then pick a fault, then press and release talk.</p>
      <div className="row">{FAULTS.map(([label, fault]) => button(label, { action: "stream_fault", fault }))}</div>
      <h3>Faults</h3>
      <div className="row">
        {button("Corrupt 3 frames", { action: "corrupt_frames" })}
        {button("Oversized frame", { action: "oversize" })}
        {button("Boot text noise", { action: "garbage" })}
        {button("Go silent", { action: "silence", on: true })}
        {button("Stop being silent", { action: "silence", on: false })}
        {button("Pull the cable", { action: "unplug" })}
        {button("Plug the cable", { action: "plug" })}
      </div>
      <MicControl />
      {sent && (
        <p className="hint" role="status">
          Sent: {sent}
        </p>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

/** The computer's own microphone. Development builds only; the product hears through the robot. */
function MicControl() {
  const app = useApp();
  const [error, setError] = useState<string | null>(null);
  const toggle = (on: boolean) => {
    setError(null);
    api.voiceDevMic(on).catch((failure: unknown) => setError(asFailure(failure).message));
  };
  return (
    <>
      <h3>Computer microphone (development only)</h3>
      <p className="hint">Lets you try the voice chain without a robot. Needs voice turned on. The product never uses this microphone.</p>
      <div className="row">
        <button className="secondary small" type="button" disabled={app.voice.mic_on} onClick={() => toggle(true)}>
          Microphone on
        </button>
        <button className="secondary small" type="button" disabled={!app.voice.mic_on} onClick={() => toggle(false)}>
          Microphone off and send
        </button>
      </div>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </>
  );
}
