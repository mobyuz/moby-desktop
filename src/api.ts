import { Channel, invoke } from "@tauri-apps/api/core";
import type { AgentEvent, Installation, Language, Profile, UsageInfo } from "./types";

/** Every command runs in src-tauri/src/api.rs. Tokens never reach this side. */

export interface ApiFailure {
  code: string;
  message: string;
}

export function asFailure(error: unknown): ApiFailure {
  if (typeof error === "object" && error !== null && "code" in error && "message" in error) {
    return error as ApiFailure;
  }
  return { code: "unexpected", message: "Something went wrong. Try again." };
}

export interface TurnMessage {
  kind: "user" | "assistant";
  text: string;
}

export const api = {
  restoreSession: () => invoke<Profile | null>("restore_session"),
  signIn: (email: string, password: string) => invoke<Profile>("sign_in", { email, password }),
  register: (email: string, password: string) => invoke<void>("register", { email, password }),
  forgotPassword: (email: string) => invoke<void>("forgot_password", { email }),
  signOut: () => invoke<void>("sign_out"),
  getUsage: () => invoke<UsageInfo>("get_usage"),
  listInstallations: () => invoke<Installation[]>("list_installations"),
  revokeInstallation: (id: string) => invoke<void>("revoke_installation", { id }),
  updateProfile: (displayName: string | null) => invoke<Profile>("update_profile", { displayName }),
  deleteAccount: (password: string) => invoke<void>("delete_account", { password }),
  exportAccount: () => invoke<string>("export_account"),
  agentTurn: (messages: TurnMessage[], language: Language, onEvent: (event: AgentEvent) => void) => {
    const channel = new Channel<AgentEvent>();
    channel.onmessage = onEvent;
    const request = { messages, available_tools: [], resources: [], language, script: language === "uz" ? "latn" : null };
    return invoke<void>("agent_turn", { request, onEvent: channel });
  },
};
