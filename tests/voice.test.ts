import { readFileSync, readdirSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { canStopVoice, NO_VOICE, retentionNote, VOICE_CONSENT_TEXT, voiceBusy, voiceFace, voiceStatusText } from "../src/voice";
import { shownExpression, NO_ROBOT } from "../src/robot";
import type { VoiceStage, VoiceView } from "../src/types";

const view = (stage: VoiceStage, patch: Partial<VoiceView> = {}): VoiceView => ({ ...NO_VOICE, stage, ...patch });

describe("voice face", () => {
  it("follows listening, thinking, speaking and error, and rests when idle", () => {
    expect(voiceFace(view("listening"))).toBe("listening");
    for (const stage of ["transcribing", "awaiting_turn", "thinking", "synthesizing"] as VoiceStage[]) expect(voiceFace(view(stage))).toBe("thinking");
    expect(voiceFace(view("speaking"))).toBe("speaking");
    expect(voiceFace(view("error"))).toBe("error");
    expect(voiceFace(view("idle"))).toBeNull();
  });

  it("never overrides the hardware mute of a live robot", () => {
    const robot = { ...NO_ROBOT, phase: "ready" as const, muted: true };
    expect(shownExpression(voiceFace(view("speaking")) ?? "idle", true, robot)).toBe("muted");
  });

  it("offers Stop whenever something can be stopped", () => {
    expect(canStopVoice(view("idle"))).toBe(false);
    expect(canStopVoice(view("error"))).toBe(false);
    for (const stage of ["listening", "transcribing", "awaiting_turn", "thinking", "synthesizing", "speaking"] as VoiceStage[]) {
      expect(canStopVoice(view(stage))).toBe(true);
      expect(voiceBusy(view(stage))).toBe(true);
    }
  });

  it("shows the notice from Rust before the stage text", () => {
    expect(voiceStatusText(view("error", { message: "Part of what Moby heard was lost." }))).toBe("Part of what Moby heard was lost.");
    expect(voiceStatusText(view("listening"))).toBe("Listening…");
  });
});

describe("voice privacy text", () => {
  it("says that audio leaves the computer, that nothing is sent before consent, and what is kept", () => {
    expect(VOICE_CONSENT_TEXT).toContain("sends what you say (as audio) to its server");
    expect(VOICE_CONSENT_TEXT).toContain("Nothing is sent until you turn voice on");
    expect(retentionNote(30)).toContain("for 30 days");
    expect(retentionNote(1)).toContain("for 1 day.");
    expect(retentionNote(null)).toContain("until you delete it");
    expect(retentionNote(30)).toContain("Audio is never saved");
  });
});

describe("the page cannot approve with a voice, and has no audio path", () => {
  const sources = (dir: string): string[] =>
    readdirSync(dir, { withFileTypes: true }).flatMap((entry) => (entry.isDirectory() ? sources(`${dir}/${entry.name}`) : entry.name.endsWith(".ts") || entry.name.endsWith(".tsx") ? [`${dir}/${entry.name}`] : []));

  it("never answers an approval from a voice event", () => {
    const store = readFileSync("src/store.tsx", "utf8");
    const voiceEffect = store.slice(store.indexOf("takenVoiceTurn"), store.indexOf("const value = useMemo"));
    expect(voiceEffect).not.toContain("answerApproval");
    expect(readFileSync("src/voice.ts", "utf8")).not.toContain("answerApproval");
  });

  it("uses no browser audio or microphone API", () => {
    for (const file of sources("src")) {
      const text = readFileSync(file, "utf8");
      expect(text, file).not.toMatch(/getUserMedia|AudioContext|MediaRecorder|SpeechRecognition|speechSynthesis/);
    }
  });

  it("gives the webview no capability", () => {
    const capability = JSON.parse(readFileSync("src-tauri/capabilities/default.json", "utf8")) as { permissions: string[] };
    expect(capability.permissions).toEqual([]);
  });
});

