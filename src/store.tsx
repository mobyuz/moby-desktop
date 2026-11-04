import { EXPRESSIONS, type Expression } from "@mobyuz/moby-protocol";
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, asFailure, type ApiFailure } from "./api";
import { loadLanguage } from "./prefs";
import type { AgentEvent, ChatMessage, Installation, Profile, UsageInfo } from "./types";

export type Status = "starting" | "offline" | "signedOut" | "signedIn";

/** The API keeps no conversation, so each turn resends the recent part of it. */
const HISTORY_LIMIT = 40;

export interface AppState {
  status: Status;
  profile: Profile | null;
  usage: UsageInfo | null;
  installations: Installation[];
  messages: ChatMessage[];
  live: string | null;
  sending: boolean;
  chatError: string | null;
  expression: Expression;
  retryStart(): void;
  signIn(email: string, password: string): Promise<void>;
  signOut(): Promise<void>;
  send(text: string): void;
  retryTurn(): void;
  saveName(name: string): Promise<void>;
  revokeInstallation(id: string): Promise<void>;
  deleteAccount(password: string): Promise<void>;
}

const Context = createContext<AppState | null>(null);

export function useApp(): AppState {
  const value = useContext(Context);
  if (!value) throw new Error("useApp must be used inside AppProvider");
  return value;
}

function isExpression(value: string): value is Expression {
  return (EXPRESSIONS as readonly string[]).includes(value);
}

export function AppProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<Status>("starting");
  const [profile, setProfile] = useState<Profile | null>(null);
  const [usage, setUsage] = useState<UsageInfo | null>(null);
  const [installations, setInstallations] = useState<Installation[]>([]);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [live, setLive] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [chatError, setChatError] = useState<string | null>(null);
  const [expression, setExpression] = useState<Expression>("idle");
  const settle = useRef<number | undefined>(undefined);

  const leave = useCallback(() => {
    setStatus("signedOut");
    setProfile(null);
    setUsage(null);
    setInstallations([]);
    setMessages([]);
    setLive(null);
    setChatError(null);
    setExpression("idle");
  }, []);

  /** A session that ended on the server sends the person back to sign-in from anywhere. */
  const guard = useCallback(
    async <T,>(action: () => Promise<T>): Promise<T> => {
      try {
        return await action();
      } catch (error) {
        const failure = asFailure(error);
        if (failure.code === "signed_out") leave();
        throw failure;
      }
    },
    [leave],
  );

  const refreshAccount = useCallback(async () => {
    const [nextUsage, nextInstallations] = await guard(() => Promise.all([api.getUsage(), api.listInstallations()]));
    setUsage(nextUsage);
    setInstallations(nextInstallations);
  }, [guard]);

  const enter = useCallback(
    (next: Profile) => {
      setProfile(next);
      setStatus("signedIn");
      refreshAccount().catch(() => undefined);
    },
    [refreshAccount],
  );

  const start = useCallback(() => {
    setStatus("starting");
    api
      .restoreSession()
      .then((restored) => {
        if (restored) enter(restored);
        else setStatus("signedOut");
      })
      .catch((error: unknown) => setStatus(asFailure(error).code === "offline" ? "offline" : "signedOut"));
  }, [enter]);

  useEffect(start, [start]);
  useEffect(() => () => window.clearTimeout(settle.current), []);

  const settleFace = useCallback((next: Expression) => {
    setExpression(next);
    window.clearTimeout(settle.current);
    settle.current = window.setTimeout(() => setExpression("idle"), 1800);
  }, []);

  const failTurn = useCallback((failure: ApiFailure) => {
    setLive(null);
    setChatError(failure.message);
    setExpression(failure.code === "offline" ? "disconnected" : "error");
  }, []);

  const runTurn = useCallback(
    (history: ChatMessage[]) => {
      setSending(true);
      setChatError(null);
      setLive(null);
      window.clearTimeout(settle.current);
      setExpression("thinking");
      const reply = (text: string) => setMessages((list) => [...list, { id: crypto.randomUUID(), role: "moby", text }]);

      const onEvent = (event: AgentEvent) => {
        if (event.event === "speech_delta") {
          setExpression("speaking");
          setLive((text) => (text ?? "") + event.text);
        } else if (event.event === "completed") {
          setLive(null);
          reply(event.display_text ?? event.speech_text);
          settleFace(isExpression(event.expression_hint) ? event.expression_hint : "happy");
        } else if (event.event === "tool_call") {
          setLive(null);
          reply("I can't do that from this app yet.");
          settleFace("confused");
        } else {
          failTurn(event.error);
        }
      };

      const turn = history.slice(-HISTORY_LIMIT).map((item) => ({ kind: item.role === "user" ? ("user" as const) : ("assistant" as const), text: item.text }));
      guard(() => api.agentTurn(turn, loadLanguage(), onEvent))
        .then(() => refreshAccount().catch(() => undefined))
        .catch((failure: ApiFailure) => failTurn(failure))
        .finally(() => setSending(false));
    },
    [failTurn, guard, refreshAccount, settleFace],
  );

  const value = useMemo<AppState>(
    () => ({
      status,
      profile,
      usage,
      installations,
      messages,
      live,
      sending,
      chatError,
      expression,
      retryStart: start,
      signIn: async (email, password) => {
        try {
          enter(await api.signIn(email, password));
        } catch (error) {
          throw asFailure(error);
        }
      },
      signOut: async () => {
        await api.signOut().catch(() => undefined);
        leave();
      },
      send: (text) => {
        const trimmed = text.trim();
        if (trimmed === "" || sending) return;
        const next = [...messages, { id: crypto.randomUUID(), role: "user" as const, text: trimmed }];
        setMessages(next);
        runTurn(next);
      },
      retryTurn: () => {
        if (!sending) runTurn(messages);
      },
      saveName: async (name) => {
        const trimmed = name.trim();
        setProfile(await guard(() => api.updateProfile(trimmed === "" ? null : trimmed)));
      },
      revokeInstallation: async (id) => {
        await guard(() => api.revokeInstallation(id));
        setInstallations(await guard(() => api.listInstallations()));
      },
      deleteAccount: async (password) => {
        await guard(() => api.deleteAccount(password));
        leave();
      },
    }),
    [chatError, enter, expression, guard, installations, leave, live, messages, profile, runTurn, sending, start, status, usage],
  );

  return <Context.Provider value={value}>{children}</Context.Provider>;
}
