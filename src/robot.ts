import type { Expression } from "@mobyuz/moby-protocol";
import type { RobotPhase, RobotSnapshot } from "./types";

/** What the app assumes before Rust has said anything. */
export const NO_ROBOT: RobotSnapshot = {
  phase: "disconnected",
  message: null,
  muted: false,
  expression: "disconnected",
  device: null,
  pair_step: null,
  pair_failure: null,
  error: null,
  retry_in_ms: null,
  port: null,
  simulated: false,
  stats: {
    frames_ok: 0,
    bad_crc: 0,
    bad_frame: 0,
    oversized: 0,
    bad_frame_version: 0,
    malformed_json: 0,
    wrong_sender: 0,
    sequence_gaps: 0,
    audio_frames_ignored: 0,
    audio_events_ignored: 0,
    audio_streams: 0,
    audio_aborted: 0,
    unexpected_messages: 0,
  },
};

/** The robot has said hello and is talking, so its face and its mute switch mean something. */
const LIVE: readonly RobotPhase[] = ["unpaired", "pairing", "foreign_paired", "key_rejected", "vault_unavailable", "pairing_reset", "unproven", "ready"];

export function isLive(robot: RobotSnapshot): boolean {
  return LIVE.includes(robot.phase);
}

export type RobotStatus = "connected" | "disconnected" | "muted" | "pairing" | "error";

/** The one word for the status chip. The hardware mute outranks everything else that is true of a live link. */
export function robotStatus(robot: RobotSnapshot): RobotStatus {
  if (!isLive(robot)) {
    return robot.phase === "incompatible" ? "error" : "disconnected";
  }
  if (robot.muted) return "muted";
  if (robot.phase === "pairing") return "pairing";
  if (robot.phase === "pairing_reset" || robot.phase === "unproven" || robot.phase === "key_rejected" || robot.phase === "vault_unavailable" || robot.error !== null) return "error";
  return "connected";
}

export const STATUS_LABEL: Record<RobotStatus, string> = {
  connected: "Connected",
  disconnected: "Disconnected",
  muted: "Muted",
  pairing: "Pairing",
  error: "Error",
};

/** A sentence under the chip that says what is going on, in the app's words. */
export function robotDetail(robot: RobotSnapshot): string {
  switch (robot.phase) {
    case "disconnected":
      return robot.message ?? "Moby isn't connected. Plug it in with a USB cable.";
    case "connecting":
      return "Connecting to Moby…";
    case "reconnecting":
      return robot.message ?? "Reconnecting to Moby…";
    case "incompatible":
    case "foreign_paired":
    case "key_rejected":
    case "vault_unavailable":
    case "pairing_reset":
    case "unproven":
      return robot.message ?? "";
    case "unpaired":
      return robot.muted ? "Not paired with this computer. The mute switch is on." : "Not paired with this computer yet.";
    case "pairing":
      return "Pairing in progress.";
    case "ready":
      return robot.muted ? "Connected. The mute switch is on, so Moby can't hear anything." : "Connected and paired with this computer.";
  }
}

/** The face the robot link asks for, or null when there is no live robot to follow. */
export function robotFace(robot: RobotSnapshot): Expression | null {
  if (!isLive(robot)) return null;
  return robot.muted ? "muted" : robot.expression;
}

/**
 * The virtual face. A muted robot always wins: the hardware switch is the truth about whether
 * Moby can hear. Otherwise a chat turn in progress shows its own mood, and when the chat is
 * quiet the face follows the robot, if there is one.
 */
export function shownExpression(chat: Expression, chatBusy: boolean, robot: RobotSnapshot): Expression {
  const face = robotFace(robot);
  if (face === null) return chat;
  if (face === "muted") return "muted";
  if (chatBusy || chat !== "idle") return chat;
  return face;
}

/** Counts worth showing: what the link threw away. */
export function droppedFrames(robot: RobotSnapshot): number {
  const s = robot.stats;
  return s.bad_crc + s.bad_frame + s.oversized + s.bad_frame_version + s.malformed_json + s.wrong_sender;
}

/** The pairing window in seconds, from the protocol's 30 s maximum. */
export const PAIR_SECONDS = 30;
