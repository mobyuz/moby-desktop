// Shapes mirror moby-api's openapi.json.

import type { Expression } from "@mobyuz/moby-protocol";

export interface Profile {
  id: string;
  email: string;
  display_name: string | null;
  installation_id: string;
}

export interface UsageInfo {
  llm_tokens_remaining: number;
  reset_at: string;
}

export interface Installation {
  id: string;
  name: string;
  os: string;
  app_version: string | null;
  revoked: boolean;
}

export type Language = "uz" | "ru" | "en";

export interface ChatMessage {
  id: string;
  role: "user" | "moby";
  text: string;
}

/** A message as sent to /v1/agent/turn. */
export interface TurnMessage {
  kind: "user" | "assistant";
  text: string;
}

/** Device settings kept by Rust. `retention_days` null keeps history until it is deleted. */
export interface Settings {
  language: Language;
  retention_days: number | null;
}

export interface ConversationSummary {
  id: string;
  title: string;
  /** Unix milliseconds. */
  updated_at: number;
}

export type ToolName = "git_status" | "open_app" | "open_project" | "search_files" | "create_reminder";

/** Events from the Rust turn loop. The tool request itself never reaches the webview. */
export type AgentEvent =
  | { event: "speech_delta"; text: string }
  | { event: "reset_speech" }
  | {
      event: "approval_required";
      approval_id: string;
      tool: ToolName;
      /** Written by the app. */
      title: string;
      details: string[];
      /** What the model said alongside the request. Untrusted. */
      moby_says: string | null;
      timeout_ms: number;
    }
  /** A launch is confirmed in a native window that Rust owns. The page only shows that it is waiting. */
  | { event: "native_prompt"; title: string }
  | { event: "native_prompt_closed" }
  | { event: "approval_closed"; approval_id: string; outcome: "approved" | "denied" | "expired" | "canceled" }
  | { event: "tool_result"; tool: ToolName; description: string; outcome: string; note: string }
  | { event: "completed"; speech_text: string; display_text: string | null; expression_hint: string }
  | { event: "error"; error: { code: string; message: string } };

/** Only reminders are answered in the page. Launches are confirmed in a native window, so there is nothing to approve here. */
export type Decision = "deny" | "once";

export interface Approval {
  id: string;
  tool: ToolName;
  title: string;
  details: string[];
  mobySays: string | null;
  timeoutMs: number;
}

export interface Project {
  id: string;
  name: string;
  /** The last two parts of the location. The full path never reaches the page. */
  short_path: string;
  created_at: number;
}

export interface Program {
  id: string;
  name: string;
  short_path: string;
  created_at: number;
}

export interface Grant {
  id: string;
  tool: ToolName;
  resource_id: string;
  level: string;
  expires_at: number | null;
  created_at: number;
}

export interface SessionGrant {
  user: string;
  tool: ToolName;
  key: string;
}

export interface Registry {
  projects: Project[];
  apps: Program[];
  grants: Grant[];
  session_grants: SessionGrant[];
}

export interface Reminder {
  id: string;
  text: string;
  due_at: number;
  state: "pending" | "done" | "missed" | "cancelled";
  created_at: number;
}

export interface ActivityEntry {
  id: number;
  tool: string;
  resource_id: string | null;
  decision: string;
  at: number;
  duration_ms: number | null;
  result_code: string | null;
}

// The robot link. Shapes mirror src-tauri/src/robot/view.rs. No key, proof or nonce is ever in them.

export type RobotPhase =
  | "disconnected"
  | "connecting"
  | "reconnecting"
  | "incompatible"
  | "unpaired"
  | "pairing"
  | "foreign_paired"
  | "key_rejected"
  | "vault_unavailable"
  | "ready";

export type PairStep = "starting" | "press_button" | "saving";

export interface RobotDevice {
  id: string;
  name: string;
  hw_rev: string;
  fw_version: string;
  paired_here: boolean;
  paired_on_robot: boolean;
  audio: boolean;
}

export interface RobotStats {
  frames_ok: number;
  bad_crc: number;
  bad_frame: number;
  oversized: number;
  bad_frame_version: number;
  malformed_json: number;
  wrong_sender: number;
  sequence_gaps: number;
  audio_frames_ignored: number;
  audio_events_ignored: number;
  unexpected_messages: number;
}

export interface RobotSnapshot {
  phase: RobotPhase;
  message: string | null;
  muted: boolean;
  expression: Expression;
  device: RobotDevice | null;
  pair_step: PairStep | null;
  pair_failure: { code: string; message: string } | null;
  error: { code: string; detail: string } | null;
  retry_in_ms: number | null;
  port: string | null;
  simulated: boolean;
  stats: RobotStats;
}

export interface RobotPort {
  name: string;
  kind: "moby" | "other_usb";
  label: string;
  simulated: boolean;
}

export interface PairedRobot {
  id: string;
  name: string;
  created_at: number;
}

export type RobotDevAction =
  | { action: "attach"; fresh: boolean; paired_elsewhere: boolean; incompatible: boolean }
  | { action: "detach" | "press" | "release" | "long_press" | "unplug" | "plug" | "corrupt_frames" | "oversize" | "garbage" }
  | { action: "mute"; muted: boolean }
  | { action: "show"; expression: Expression }
  | { action: "silence"; on: boolean };
