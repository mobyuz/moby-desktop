import type { ActivityEntry, Approval, AgentEvent, Grant, Registry, SessionGrant, ToolName } from "./types";

/** Safe default of the approval dialog: it starts on this, and no timeout or key press approves. */
export const SAFE_DECISION = "deny" as const;

/** Approval buttons that allow something stay off for a moment so a stray key or click cannot approve. */
export const ARM_DELAY_MS = 600;

export const TOOL_LABELS: Record<string, string> = {
  git_status: "Git status",
  search_files: "File search",
  open_app: "Open program",
  open_project: "Open project",
  create_reminder: "Create reminder",
  reminder_due: "Reminder shown",
  unknown: "Unrecognised request",
};

export const DECISION_LABELS: Record<string, string> = {
  auto: "Allowed by a saved permission",
  session_grant: "Allowed for this session",
  approved_once: "You allowed it once",
  approved_session: "You allowed it for this session",
  denied: "You denied it",
  expired: "No answer, so denied",
  canceled: "Canceled",
  rejected: "Rejected as an invalid request",
  no_grant: "Blocked: no permission",
  unknown_resource: "Blocked: not in your list",
  changed: "Blocked: it changed while waiting",
  round_limit: "Stopped: too many steps",
  scheduled: "Shown on time",
  error: "Could not be checked",
};

export const OUTCOME_LABELS: Record<string, string> = {
  ok: "Done",
  denied: "Not allowed",
  invalid_request: "Not valid",
  unavailable: "Not available",
  canceled: "Canceled",
  timeout: "Took too long",
  failed: "Failed",
  partial_result: "Partial result",
};

export function toolLabel(tool: string): string {
  return TOOL_LABELS[tool] ?? "Unrecognised request";
}

/** The line shown in the chat after a tool call. The text comes from the app, not from the model. */
export function toolNote(event: Extract<AgentEvent, { event: "tool_result" }>): string {
  return event.outcome === "ok" ? `${event.description}: done.` : `${event.description}: ${event.note || OUTCOME_LABELS[event.outcome] || "failed"}.`;
}

export function approvalFrom(event: Extract<AgentEvent, { event: "approval_required" }>): Approval {
  return {
    id: event.approval_id,
    tool: event.tool,
    title: event.title,
    details: event.details,
    mobySays: event.moby_says,
    allowSession: event.allow_session,
    timeoutMs: event.timeout_ms,
  };
}

export type ReadTool = Extract<ToolName, "git_status" | "search_files">;

export const READ_TOOLS: { tool: ReadTool; label: string }[] = [
  { tool: "git_status", label: "Git status" },
  { tool: "search_files", label: "Search file names" },
];

/** A search root has the same folder as its project. */
export function resourceFor(tool: ReadTool, projectId: string): string {
  return tool === "git_status" ? projectId : projectId.replace(/^proj_/, "root_");
}

export type Access = { kind: "always"; grant: Grant } | { kind: "session"; grant: SessionGrant } | { kind: "none" };

export function accessFor(registry: Registry, tool: ReadTool, projectId: string): Access {
  const resource = resourceFor(tool, projectId);
  const grant = registry.grants.find((item) => item.tool === tool && item.resource_id === resource);
  if (grant) return { kind: "always", grant };
  const session = registry.session_grants.find((item) => item.tool === tool && item.key === resource);
  return session ? { kind: "session", grant: session } : { kind: "none" };
}

/** Names for the ids in the activity log. Removed items show as removed, never as an id. */
export function resourceName(registry: Registry, id: string | null): string {
  if (id === null) return "";
  const project = registry.projects.find((item) => item.id === id || item.id.replace(/^proj_/, "root_") === id);
  if (project) return project.name;
  return registry.apps.find((item) => item.id === id)?.name ?? "(removed)";
}

export function sessionGrantLabel(registry: Registry, grant: SessionGrant): string {
  const [first, second] = grant.key.split("+");
  const parts = [first, second].filter((part): part is string => part !== undefined && part !== "explorer").map((part) => resourceName(registry, part));
  return `${toolLabel(grant.tool)}: ${parts.join(" in ") || "Windows Explorer"}`;
}

export function activityLine(entry: ActivityEntry): string {
  const decision = DECISION_LABELS[entry.decision] ?? entry.decision;
  const result = entry.result_code ? OUTCOME_LABELS[entry.result_code] ?? entry.result_code : "";
  return result && entry.result_code !== "ok" ? `${decision}. ${result}.` : `${decision}.`;
}

const REMINDER_STATES: Record<string, string> = { pending: "Waiting", done: "Shown", missed: "Missed", cancelled: "Canceled" };

export const reminderState = (state: string) => REMINDER_STATES[state] ?? state;
