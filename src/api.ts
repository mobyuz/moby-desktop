import { Channel, invoke } from "@tauri-apps/api/core";
import type { ActivityEntry, AgentEvent, ChatMessage, ConversationSummary, Decision, Installation, Language, PairedRobot, Profile, Program, Project, Registry, Reminder, RobotDevAction, RobotPort, RobotSnapshot, Settings, ToolName, TurnMessage, UsageInfo } from "./types";

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
    // Only the conversation text and the language go to Rust. It decides which tools and resources exist.
    return invoke<void>("agent_turn", { turn: { messages, language }, onEvent: channel });
  },
  /** Answers a dialog that Rust opened. The id comes from an approval_required event. */
  answerApproval: (approvalId: string, decision: Decision) => invoke<void>("answer_approval", { approvalId, decision }),
  cancelTurn: () => invoke<void>("cancel_turn"),

  // Tools and projects. Folders and programs are picked in a native dialog that Rust opens.
  listRegistry: () => invoke<Registry>("list_registry"),
  addProject: () => invoke<Project | null>("add_project"),
  addApp: () => invoke<Program | null>("add_app"),
  removeProject: (id: string) => invoke<void>("remove_project", { id }),
  removeApp: (id: string) => invoke<void>("remove_app", { id }),
  grantRead: (projectId: string, tool: ToolName, sessionOnly: boolean) => invoke<void>("grant_read", { projectId, tool, sessionOnly }),
  revokeGrant: (grantId: string) => invoke<void>("revoke_grant", { grantId }),
  revokeSessionGrant: (tool: ToolName, key: string) => invoke<void>("revoke_session_grant", { tool, key }),
  listReminders: () => invoke<Reminder[]>("list_reminders"),
  deleteReminder: (id: string) => invoke<void>("delete_reminder", { id }),
  listActivity: () => invoke<ActivityEntry[]>("list_activity"),
  clearActivity: () => invoke<number>("clear_activity"),

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

  // The robot link. Pairing has no arguments: the key is made, kept and used in Rust only.
  robotSubscribe: (onChange: (snapshot: RobotSnapshot) => void) => {
    const channel = new Channel<RobotSnapshot>();
    channel.onmessage = onChange;
    return invoke<void>("robot_subscribe", { onEvent: channel });
  },
  robotPorts: () => invoke<RobotPort[]>("robot_ports"),
  /** No port means the one that looks like a Moby. A port must be one that robotPorts listed. */
  robotConnect: (port?: string) => invoke<void>("robot_connect", { port: port ?? null }),
  robotDisconnect: () => invoke<void>("robot_disconnect"),
  robotBeginPairing: () => invoke<void>("robot_begin_pairing"),
  robotCancelPairing: () => invoke<void>("robot_cancel_pairing"),
  robotDevices: () => invoke<PairedRobot[]>("robot_devices"),
  robotUnpair: (deviceId: string) => invoke<void>("robot_unpair", { deviceId }),
  /** The simulator panel. Development builds only; a release build answers not_available. */
  robotDev: (action: RobotDevAction) => invoke<void>("robot_dev", { action }),
};
