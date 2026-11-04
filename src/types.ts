// Shapes mirror moby-api's openapi.json so the demo layer can be swapped for the real client in D1.

export interface Profile {
  id: string;
  email: string;
  display_name: string | null;
  installation_id: string;
}

export interface UsageInfo {
  llm_tokens_remaining: number;
  stt_seconds_remaining: number;
  tts_chars_remaining: number;
  reset_at: string;
}

export interface Installation {
  id: string;
  name: string;
  os: string;
  app_version: string | null;
  revoked: boolean;
}

export type ConsentKind = "terms" | "privacy_policy" | "cloud_voice";

export interface ConsentRecord {
  type: ConsentKind;
  version: string;
  accepted_at: string;
  revoked_at: string | null;
}

export type ToolName = "git_status" | "open_app" | "open_project" | "search_files" | "create_reminder";

export type ApprovalStatus = "pending" | "approved" | "denied" | "blocked";

export interface Approval {
  id: string;
  tool: ToolName;
  title: string;
  detail: string;
  status: ApprovalStatus;
  at: string;
}

export interface ChatMessage {
  id: string;
  role: "user" | "moby" | "system";
  text: string;
  at: string;
}

export interface Device {
  id: string;
  name: string;
  firmware: string;
  protocol: number;
  connected: boolean;
  hardwareMuted: boolean;
  pairedAt: string;
}

export interface Project {
  id: string;
  name: string;
  path: string;
}

export interface LogLine {
  at: string;
  level: "info" | "warn" | "error";
  text: string;
}

export type SpeechLanguage = "uz" | "ru" | "en";
