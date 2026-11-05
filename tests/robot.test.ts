import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { droppedFrames, isLive, NO_ROBOT, robotDetail, robotFace, robotStatus, shownExpression } from "../src/robot";
import type { RobotPhase, RobotSnapshot } from "../src/types";

const robot = (patch: Partial<RobotSnapshot>): RobotSnapshot => ({ ...NO_ROBOT, ...patch });

describe("robot status", () => {
  it("is disconnected until the robot has said hello", () => {
    for (const phase of ["disconnected", "connecting", "reconnecting"] as RobotPhase[]) {
      expect(robotStatus(robot({ phase }))).toBe("disconnected");
      expect(isLive(robot({ phase }))).toBe(false);
    }
  });

  it("reports the five states the interface shows", () => {
    expect(robotStatus(robot({ phase: "ready" }))).toBe("connected");
    expect(robotStatus(robot({ phase: "unpaired" }))).toBe("connected");
    expect(robotStatus(robot({ phase: "pairing", pair_step: "press_button" }))).toBe("pairing");
    expect(robotStatus(robot({ phase: "ready", error: { code: "E_HW", detail: "x" } }))).toBe("error");
    expect(robotStatus(robot({ phase: "key_rejected" }))).toBe("error");
    expect(robotStatus(robot({ phase: "incompatible" }))).toBe("error");
  });

  it("lets the hardware mute outrank every other state of a live link", () => {
    expect(robotStatus(robot({ phase: "pairing", muted: true }))).toBe("muted");
    expect(robotStatus(robot({ phase: "ready", muted: true, error: { code: "E_HW", detail: "x" } }))).toBe("muted");
  });

  it("explains each phase in words and never leaves a state blank", () => {
    const phases: RobotPhase[] = ["disconnected", "connecting", "reconnecting", "incompatible", "unpaired", "pairing", "foreign_paired", "key_rejected", "vault_unavailable", "pairing_reset", "unproven", "ready"];
    for (const phase of phases) {
      const message = phase === "incompatible" || phase === "foreign_paired" || phase === "key_rejected" || phase === "vault_unavailable" || phase === "pairing_reset" || phase === "unproven" ? "Because." : null;
      expect(robotDetail(robot({ phase, message })).length).toBeGreaterThan(0);
    }
    expect(robotDetail(robot({ phase: "ready", muted: true }))).toContain("mute switch is on");
  });

  it("counts what the link dropped", () => {
    const stats = { ...NO_ROBOT.stats, bad_crc: 2, oversized: 1, malformed_json: 3, audio_frames_ignored: 9 };
    expect(droppedFrames(robot({ stats }))).toBe(6);
  });
});

describe("the face follows the robot", () => {
  it("uses the chat's face when there is no live robot", () => {
    expect(robotFace(NO_ROBOT)).toBeNull();
    expect(shownExpression("thinking", true, NO_ROBOT)).toBe("thinking");
    expect(shownExpression("idle", false, robot({ phase: "reconnecting", expression: "listening" }))).toBe("idle");
  });

  it("follows the robot's state while the chat is quiet", () => {
    expect(shownExpression("idle", false, robot({ phase: "ready", expression: "listening" }))).toBe("listening");
    expect(shownExpression("idle", false, robot({ phase: "unpaired", expression: "error" }))).toBe("error");
  });

  it("keeps the chat's mood during a chat turn", () => {
    expect(shownExpression("thinking", true, robot({ phase: "ready", expression: "idle" }))).toBe("thinking");
    expect(shownExpression("happy", false, robot({ phase: "ready", expression: "idle" }))).toBe("happy");
  });

  it("always shows muted for a muted robot, even mid-chat", () => {
    const muted = robot({ phase: "ready", muted: true, expression: "idle" });
    expect(shownExpression("speaking", true, muted)).toBe("muted");
    expect(robotFace(muted)).toBe("muted");
  });
});

describe("robot link boundary", () => {
  const api = readFileSync(new URL("../src/api.ts", import.meta.url), "utf8");
  const conf = readFileSync(new URL("../src-tauri/capabilities/default.json", import.meta.url), "utf8");

  it("has no way to send a key from the page", () => {
    expect(api).not.toMatch(/pair_?key|robot_set_key|proof|nonce/i);
    const pairing = api.match(/robotBeginPairing:[^\n]*/)?.[0] ?? "";
    expect(pairing).toContain('invoke<void>("robot_begin_pairing")');
  });

  it("adds no webview permissions", () => {
    expect(JSON.parse(conf).permissions).toEqual([]);
  });
});
