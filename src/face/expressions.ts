import type { Expression } from "@mobyuz/moby-protocol";

export type FaceAnimation =
  | "blink"
  | "pulse"
  | "orbit"
  | "talk"
  | "bounce"
  | "wobble"
  | "shake"
  | "breathe"
  | "flicker";

/** The red LED marks an audio session, the green LED an open microphone path. */
export type LedState = "off" | "on" | "pulse";

export interface ExpressionMeta {
  label: string;
  status: string;
  animation: FaceAnimation;
  redLed: LedState;
  greenLed: LedState;
}

export const EXPRESSION_META: Record<Expression, ExpressionMeta> = {
  idle: { label: "Idle", status: "Ready when you are.", animation: "blink", redLed: "off", greenLed: "on" },
  listening: { label: "Listening", status: "Listening to you.", animation: "pulse", redLed: "pulse", greenLed: "on" },
  thinking: { label: "Thinking", status: "Working on it.", animation: "orbit", redLed: "off", greenLed: "on" },
  speaking: { label: "Speaking", status: "Answering.", animation: "talk", redLed: "off", greenLed: "on" },
  happy: { label: "Happy", status: "Done.", animation: "bounce", redLed: "off", greenLed: "on" },
  confused: { label: "Confused", status: "I did not understand that.", animation: "wobble", redLed: "off", greenLed: "on" },
  error: { label: "Error", status: "Something went wrong.", animation: "shake", redLed: "off", greenLed: "on" },
  muted: { label: "Muted", status: "Microphone is off.", animation: "breathe", redLed: "off", greenLed: "off" },
  disconnected: {
    label: "Disconnected",
    status: "No robot or service connection.",
    animation: "flicker",
    redLed: "off",
    greenLed: "off",
  },
};
