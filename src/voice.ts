import type { Expression } from "@mobyuz/moby-protocol";
import type { VoiceView } from "./types";

/** What the app assumes before Rust has said anything. */
export const NO_VOICE: VoiceView = {
  stage: "idle",
  message: null,
  code: null,
  pending: null,
  mic_on: false,
  dev_mic: false,
};

/** The face the voice chain asks for, or null when it is resting. */
export function voiceFace(view: VoiceView): Expression | null {
  switch (view.stage) {
    case "listening":
      return "listening";
    case "transcribing":
    case "awaiting_turn":
    case "thinking":
    case "synthesizing":
      return "thinking";
    case "speaking":
      return "speaking";
    case "error":
      return "error";
    case "idle":
      return null;
  }
}

/** True while the chain is working, so the face keeps its mood instead of settling to idle. */
export function voiceBusy(view: VoiceView): boolean {
  return view.stage !== "idle" && view.stage !== "error";
}

const STAGE_TEXT: Record<VoiceView["stage"], string> = {
  idle: "Ready. Hold the talk button on Moby and speak.",
  listening: "Listening…",
  transcribing: "Turning your words into text…",
  awaiting_turn: "Got it.",
  thinking: "Thinking…",
  synthesizing: "Getting Moby's voice ready…",
  speaking: "Moby is speaking.",
  error: "Something went wrong.",
};

/** One sentence under the voice controls. A notice from Rust wins over the stage text. */
export function voiceStatusText(view: VoiceView): string {
  return view.message ?? STAGE_TEXT[view.stage];
}

/** Stop is offered whenever there is something to stop. */
export function canStopVoice(view: VoiceView): boolean {
  return view.stage !== "idle" && view.stage !== "error";
}

export const VOICE_CONSENT_TEXT =
  "To understand you, Moby sends what you say (as audio) to its server, which turns it into text and then reads Moby's answer aloud. The server does not keep the audio or the text. Nothing is sent until you turn voice on, and you can turn it off at any time.";

export function retentionNote(days: number | null): string {
  const kept = days === null ? "until you delete it" : `for ${days} day${days === 1 ? "" : "s"}`;
  return `The words you say are saved in your conversation on this computer, like typed messages, ${kept}. Audio is never saved.`;
}

export const VOICE_DISCLOSURE = "Moby's spoken answers are made by AI and played through this computer's speakers.";
