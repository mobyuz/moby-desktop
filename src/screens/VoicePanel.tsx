import { useState } from "react";
import { asFailure } from "../api";
import { useApp } from "../store";
import { canStopVoice, retentionNote, VOICE_CONSENT_TEXT, VOICE_DISCLOSURE, voiceStatusText } from "../voice";

/** Voice: the consent, what is happening now, and Stop. Audio never reaches this page. */
export function VoicePanel() {
  const app = useApp();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const accepted = app.voiceConsent?.accepted === true;
  const canTalk = app.robot.device?.audio === true;

  const change = (action: () => Promise<void>) => {
    setError(null);
    setBusy(true);
    action()
      .catch((failure: unknown) => setError(asFailure(failure).message))
      .finally(() => setBusy(false));
  };

  return (
    <section aria-labelledby="voice-title">
      <h2 id="voice-title">Voice</h2>
      <p>{VOICE_CONSENT_TEXT}</p>
      <p className="hint">{retentionNote(app.retentionDays)}</p>
      <div className="row">
        {accepted ? (
          <button className="secondary" type="button" disabled={busy} onClick={() => change(app.revokeVoice)}>
            Turn voice off
          </button>
        ) : (
          <button className="primary" type="button" disabled={busy || app.voiceConsent === null} onClick={() => change(app.acceptVoice)}>
            Turn voice on
          </button>
        )}
        {canStopVoice(app.voice) && (
          <button className="secondary" type="button" onClick={app.stop}>
            Stop
          </button>
        )}
      </div>
      <p role="status" className="robot-detail">
        {accepted ? voiceStatusText(app.voice) : "Voice is off. Nothing you say is sent anywhere."}
      </p>
      {accepted && !canTalk && <p className="hint">This Moby's firmware has no voice link, so there is nothing to listen to.</p>}
      {accepted && <p className="hint">{VOICE_DISCLOSURE}</p>}
      <p className="hint">Moby only listens while you hold its talk button. There is no wake word and it never listens on its own. The mute switch always wins.</p>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
