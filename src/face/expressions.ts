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

export interface ExpressionMeta {
  label: string;
  status: string;
  animation: FaceAnimation;
}

export const EXPRESSION_META: Record<Expression, ExpressionMeta> = {
  idle: { label: "Idle", status: "Ready", animation: "blink" },
  listening: { label: "Listening", status: "Listening", animation: "pulse" },
  thinking: { label: "Thinking", status: "Thinking", animation: "orbit" },
  speaking: { label: "Speaking", status: "Answering", animation: "talk" },
  happy: { label: "Happy", status: "Done", animation: "bounce" },
  confused: { label: "Confused", status: "Didn't understand", animation: "wobble" },
  error: { label: "Error", status: "Something went wrong", animation: "shake" },
  muted: { label: "Muted", status: "Microphone off", animation: "breathe" },
  disconnected: { label: "Disconnected", status: "Can't connect", animation: "flicker" },
};
