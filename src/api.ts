import { Channel, invoke } from "@tauri-apps/api/core";
import type { AgentEvent, ChatMessage, ConversationSummary, Installation, Language, Profile, Settings, TurnMessage, UsageInfo } from "./types";

/** Every command runs in src-tauri/src (api.rs and store.rs). Tokens and the history file never reach this side. */

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

  // Local memory. The account is the one Rust signed in, so none is passed here.
  getSettings: () => invoke<Settings>("get_settings"),
  setLanguage: (language: Language) => invoke<void>("set_language", { language }),
  /** Resolves to how many conversations the new setting removed. */
  setRetention: (days: number | null) => invoke<number>("set_retention", { days }),
  listConversations: () => invoke<ConversationSummary[]>("list_conversations"),
  createConversation: (title: string) => invoke<ConversationSummary>("create_conversation", { title }),
  loadMessages: (conversationId: string) => invoke<ChatMessage[]>("load_messages", { conversationId }),
  appendMessage: (conversationId: string, message: ChatMessage) => invoke<void>("append_message", { conversationId, message }),
  deleteConversation: (id: string) => invoke<void>("delete_conversation", { id }),
  deleteAllHistory: () => invoke<void>("delete_all_history"),
};
