// Shapes mirror moby-api's openapi.json.

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

export type AgentEvent =
  | { event: "speech_delta"; text: string }
  | { event: "tool_call" }
  | { event: "completed"; speech_text: string; display_text?: string | null; expression_hint: string }
  | { event: "error"; error: { code: string; message: string } };
