import type { ChatMessage, TurnMessage } from "./types";

/** moby-api takes at most 64 messages of 8 KiB each. The API keeps no conversation, so each turn resends the recent part. */
export const HISTORY_LIMIT = 40;
export const MAX_TEXT_BYTES = 8 * 1024;
/** Total text sent per turn, so a few long messages cannot crowd out the recent ones. */
export const CONTEXT_BYTES = 32 * 1024;

const TITLE_CHARS = 60;

const encoder = new TextEncoder();

export function utf8Length(text: string): number {
  return encoder.encode(text).length;
}

export const exceedsLimit = (text: string) => utf8Length(text) > MAX_TEXT_BYTES;

/** Cuts at a character boundary, never inside a multi-byte character. */
export function clipText(text: string, maxBytes: number): string {
  if (utf8Length(text) <= maxBytes) return text;
  let used = 0;
  let end = 0;
  for (const char of text) {
    const size = utf8Length(char);
    if (used + size > maxBytes) break;
    used += size;
    end += char.length;
  }
  return text.slice(0, end);
}

/**
 * The messages for one agent turn, from the active conversation only.
 * Newest first until a limit is reached, then restored to reading order. The newest message always goes, clipped if need be.
 */
export function buildTurn(messages: ChatMessage[]): TurnMessage[] {
  const picked: TurnMessage[] = [];
  let total = 0;
  for (let index = messages.length - 1; index >= 0 && picked.length < HISTORY_LIMIT; index--) {
    const message = messages[index];
    if (!message) continue;
    const text = clipText(message.text, MAX_TEXT_BYTES);
    const size = utf8Length(text);
    if (picked.length > 0 && total + size > CONTEXT_BYTES) break;
    total += size;
    picked.push({ kind: message.role === "user" ? "user" : "assistant", text });
  }
  picked.reverse();
  // A reply cut off from the question it answers only confuses the model.
  while (picked[0]?.kind === "assistant") picked.shift();
  return picked;
}

/** A short title for the conversation list, taken from the first message. */
export function titleFrom(text: string): string {
  const flat = text.replace(/[\p{Cc}\s]+/gu, " ").trim();
  const chars = Array.from(flat);
  return chars.length > TITLE_CHARS ? `${chars.slice(0, TITLE_CHARS).join("").trimEnd()}…` : flat || "New conversation";
}

export interface RetentionChoice {
  /** null keeps history until it is deleted. */
  days: number | null;
  label: string;
}

export const RETENTION_CHOICES: RetentionChoice[] = [
  { days: 7, label: "7 days" },
  { days: 30, label: "30 days" },
  { days: 90, label: "90 days" },
  { days: 365, label: "1 year" },
  { days: null, label: "Until I delete it" },
];

/** Dates in the conversation list. The locale is one constant so translating the interface later means changing it here. */
const DATE_LOCALE = "en";

export const formatWhen = (millis: number) => new Date(millis).toLocaleString(DATE_LOCALE, { dateStyle: "medium", timeStyle: "short" });
