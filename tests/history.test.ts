import { describe, expect, it } from "vitest";
import { buildTurn, clipText, CONTEXT_BYTES, exceedsLimit, HISTORY_LIMIT, MAX_TEXT_BYTES, titleFrom, utf8Length } from "../src/history";
import type { ChatMessage } from "../src/types";

function chat(count: number, text = "hello"): ChatMessage[] {
  return Array.from({ length: count }, (_, index) => ({
    id: String(index),
    role: index % 2 === 0 ? ("user" as const) : ("moby" as const),
    text: `${text} ${index}`,
  }));
}

describe("the messages sent for one turn", () => {
  it("sends a short conversation whole, in order, with the right kinds", () => {
    const turn = buildTurn(chat(3));
    expect(turn.map((m) => m.kind)).toEqual(["user", "assistant", "user"]);
    expect(turn.map((m) => m.text)).toEqual(["hello 0", "hello 1", "hello 2"]);
  });

  it("keeps only the newest messages and stays under the API's 64", () => {
    const turn = buildTurn(chat(100));
    expect(turn.length).toBeLessThanOrEqual(HISTORY_LIMIT);
    expect(turn.length).toBeLessThan(64);
    expect(turn.at(-1)?.text).toBe("hello 99");
  });

  it("does not start with a reply whose question was cut off", () => {
    // 100 messages, so the 40 newest are indexes 60..99: that window begins with a user message...
    expect(buildTurn(chat(100))[0]?.kind).toBe("user");
    // ...but 101 messages shifts it by one and the window would begin with a reply.
    const turn = buildTurn(chat(101));
    expect(turn[0]?.kind).toBe("user");
    expect(turn).toHaveLength(HISTORY_LIMIT - 1);
  });

  it("clips each text to the API limit without splitting a character", () => {
    const long = "\u{1F40B}".repeat(MAX_TEXT_BYTES); // 4 bytes each
    const turn = buildTurn([{ id: "1", role: "user", text: long }]);
    const sent = turn[0]?.text ?? "";
    expect(utf8Length(sent)).toBeLessThanOrEqual(MAX_TEXT_BYTES);
    expect(sent).toBe("\u{1F40B}".repeat(MAX_TEXT_BYTES / 4));
    expect(sent).not.toContain("�");
  });

  it("drops older messages once the total budget is used, but always sends the newest", () => {
    const big = "x".repeat(MAX_TEXT_BYTES);
    const messages: ChatMessage[] = Array.from({ length: 10 }, (_, index) => ({ id: String(index), role: index % 2 === 0 ? "user" : "moby", text: big }));
    const turn = buildTurn(messages);
    const total = turn.reduce((sum, m) => sum + utf8Length(m.text), 0);
    expect(total).toBeLessThanOrEqual(CONTEXT_BYTES);
    expect(turn.length).toBeGreaterThan(0);
    expect(turn.at(-1)?.kind).toBe("assistant");
  });

  it("sends nothing for an empty conversation", () => {
    expect(buildTurn([])).toEqual([]);
  });
});

describe("text limits", () => {
  it("counts bytes, not characters", () => {
    expect(exceedsLimit("a".repeat(MAX_TEXT_BYTES))).toBe(false);
    expect(exceedsLimit("a".repeat(MAX_TEXT_BYTES + 1))).toBe(true);
    expect(exceedsLimit("\u{1F40B}".repeat(MAX_TEXT_BYTES / 4 + 1))).toBe(true);
  });

  it("leaves text under the limit alone", () => {
    expect(clipText("salom", 100)).toBe("salom");
    expect(clipText("привет", 4)).toBe("пр");
  });
});

describe("conversation titles", () => {
  it("collapses whitespace and control characters", () => {
    expect(titleFrom("  hello \n\n  there\t\u0007 ")).toBe("hello there");
  });

  it("shortens long titles on a character boundary", () => {
    const title = titleFrom("\u{1F40B}".repeat(100));
    expect(Array.from(title)).toHaveLength(61);
    expect(title.endsWith("…")).toBe(true);
  });

  it("never returns an empty title", () => {
    expect(titleFrom(" \n ")).toBe("New conversation");
  });
});
