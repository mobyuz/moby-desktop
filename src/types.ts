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
      allow_session: boolean;
      timeout_ms: number;
    }
  | { event: "approval_closed"; approval_id: string; outcome: "approved" | "denied" | "expired" | "canceled" }
  | { event: "tool_result"; tool: ToolName; description: string; outcome: string; note: string }
  | { event: "completed"; speech_text: string; display_text: string | null; expression_hint: string }
  | { event: "error"; error: { code: string; message: string } };

export type Decision = "deny" | "once" | "session";

export interface Approval {
  id: string;
  tool: ToolName;
  title: string;
  details: string[];
  mobySays: string | null;
  allowSession: boolean;
  timeoutMs: number;
}

export interface Project {
  id: string;
  name: string;
  path: string;
  created_at: number;
}

export interface Program {
  id: string;
  name: string;
  path: string;
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
