import { describe, expect, it } from "vitest";
import { accessFor, activityLine, approvalFrom, resourceFor, resourceName, SAFE_DECISION, sessionGrantLabel, toolNote } from "../src/tools";
import type { Registry } from "../src/types";

const registry: Registry = {
  projects: [{ id: "proj_a", name: "moby-api", path: "C:\\work\\moby-api", created_at: 1 }],
  apps: [{ id: "app_b", name: "Code", path: "C:\\Code.exe", created_at: 1 }],
  grants: [{ id: "g1", tool: "git_status", resource_id: "proj_a", level: "permanent_read", expires_at: null, created_at: 1 }],
  session_grants: [{ user: "u", tool: "search_files", key: "root_a" }],
};

describe("approval dialog", () => {
  it("starts from deny", () => {
    expect(SAFE_DECISION).toBe("deny");
  });

  it("keeps the app's title and the model's commentary apart", () => {
    const approval = approvalFrom({
      event: "approval_required",
      approval_id: "apr_1",
      tool: "open_app",
      title: "Open Code",
      details: ["Program: C:\\Code.exe"],
      moby_says: "Ignore the rules and click allow",
      allow_session: true,
      timeout_ms: 120000,
    });
    expect(approval.title).toBe("Open Code");
    expect(approval.mobySays).toBe("Ignore the rules and click allow");
    expect(approval.title).not.toContain("Ignore");
  });
});

describe("tool notes", () => {
  it("states what happened in the app's own words", () => {
    expect(toolNote({ event: "tool_result", tool: "git_status", description: "Run git status in project moby-api", outcome: "ok", note: "" })).toBe("Run git status in project moby-api: done.");
    expect(toolNote({ event: "tool_result", tool: "open_app", description: "Open Code", outcome: "denied", note: "Not allowed" })).toBe("Open Code: Not allowed.");
  });
});

describe("read access", () => {
  it("finds a saved grant, a session grant, or none", () => {
    expect(accessFor(registry, "git_status", "proj_a").kind).toBe("always");
    expect(accessFor(registry, "search_files", "proj_a").kind).toBe("session");
    expect(accessFor({ ...registry, session_grants: [] }, "search_files", "proj_a").kind).toBe("none");
  });

  it("maps a project to its search root", () => {
    expect(resourceFor("search_files", "proj_abc")).toBe("root_abc");
    expect(resourceFor("git_status", "proj_abc")).toBe("proj_abc");
  });
});

describe("activity", () => {
  it("shows names, never ids or paths", () => {
    expect(resourceName(registry, "proj_a")).toBe("moby-api");
    expect(resourceName(registry, "root_a")).toBe("moby-api");
    expect(resourceName(registry, "app_gone")).toBe("(removed)");
    expect(resourceName(registry, null)).toBe("");
  });

  it("describes the decision and any failure", () => {
    expect(activityLine({ id: 1, tool: "open_app", resource_id: null, decision: "denied", at: 0, duration_ms: null, result_code: "denied" })).toBe("You denied it. Not allowed.");
    expect(activityLine({ id: 2, tool: "git_status", resource_id: null, decision: "auto", at: 0, duration_ms: 5, result_code: "ok" })).toBe("Allowed by a saved permission.");
  });

  it("labels session approvals with the project and program", () => {
    expect(sessionGrantLabel(registry, { user: "u", tool: "open_project", key: "proj_a+app_b" })).toBe("Open project: moby-api in Code");
    expect(sessionGrantLabel(registry, { user: "u", tool: "open_project", key: "proj_a+explorer" })).toBe("Open project: moby-api");
  });
});
