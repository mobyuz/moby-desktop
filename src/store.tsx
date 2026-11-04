import { EXPRESSIONS, type Expression } from "@mobyuz/moby-protocol";
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, asFailure, type ApiFailure } from "./api";
import { buildTurn, exceedsLimit, titleFrom } from "./history";
import { clearSavedLanguage, loadSavedLanguage, saveLanguageFallback } from "./prefs";
import type { AgentEvent, ChatMessage, ConversationSummary, Installation, Language, Profile, UsageInfo } from "./types";

export type Status = "starting" | "offline" | "signedOut" | "signedIn";

/** Shown whenever the history file fails. Chat keeps working from memory. */
const STORE_WARNING = "Moby can't save your history on this computer right now. Your conversations stay until you close the app.";

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
  language: Language;
  /** null keeps history until it is deleted. */
  retentionDays: number | null;
  /** Set when the history file could not be used. Null while saving works. */
  storeWarning: string | null;
  conversations: ConversationSummary[];
  activeId: string | null;
  retryStart(): void;
  signIn(email: string, password: string): Promise<void>;
  signOut(): Promise<void>;
  send(text: string): void;
  retryTurn(): void;
  saveName(name: string): Promise<void>;
  revokeInstallation(id: string): Promise<void>;
  deleteAccount(password: string): Promise<void>;
  newConversation(): void;
  openConversation(id: string): Promise<void>;
  deleteConversation(id: string): Promise<void>;
  setLanguage(language: Language): Promise<void>;
  /** Resolves to how many conversations the new setting removed. */
  setRetention(days: number | null): Promise<number>;
  deleteAllHistory(): Promise<void>;
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
  const [language, setLanguageState] = useState<Language>(() => loadSavedLanguage() ?? "uz");
  const [retentionDays, setRetentionDays] = useState<number | null>(30);
  const [storeWarning, setStoreWarning] = useState<string | null>(null);
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const settle = useRef<number | undefined>(undefined);
  /** Messages of every conversation opened this session. Also the only copy of one that could not be saved. */
  const cache = useRef(new Map<string, ChatMessage[]>());
  /** Conversations that exist only in memory because the history file failed. */
  const unsaved = useRef(new Set<string>());
  const activeRef = useRef<string | null>(null);

  /** Shows one conversation, or the empty start screen for null. */
  const activate = useCallback((id: string | null, list: ChatMessage[]) => {
    activeRef.current = id;
    setActiveId(id);
    setMessages(list);
    setLive(null);
    setChatError(null);
    setExpression("idle");
  }, []);

  const leave = useCallback(() => {
    setStatus("signedOut");
    setProfile(null);
    setUsage(null);
    setInstallations([]);
    cache.current.clear();
    unsaved.current.clear();
    setConversations([]);
    activate(null, []);
  }, [activate]);

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

  /** Saves in the background. A failure only raises the warning, so chat never stops for it. */
  const remember = useCallback(
    (action: () => Promise<unknown>) => {
      guard(action).catch((error: unknown) => {
        if (asFailure(error).code !== "signed_out") setStoreWarning(STORE_WARNING);
      });
    },
    [guard],
  );

  const loadHistory = useCallback(async () => {
    try {
      const list = await guard(() => api.listConversations());
      setConversations((current) => [...current, ...list.filter((item) => !current.some((known) => known.id === item.id))]);
      const latest = list[0];
      if (latest && activeRef.current === null) {
        const loaded = await guard(() => api.loadMessages(latest.id));
        cache.current.set(latest.id, loaded);
        if (activeRef.current === null) activate(latest.id, loaded);
      }
    } catch (error) {
      if (asFailure(error).code !== "signed_out") setStoreWarning(STORE_WARNING);
    }
  }, [activate, guard]);

  /** After retention removed rows: forget what is gone and leave a conversation that no longer exists. */
  const syncHistory = useCallback(async () => {
    const list = await guard(() => api.listConversations());
    const kept = new Set(list.map((item) => item.id));
    for (const id of [...cache.current.keys()]) if (!kept.has(id) && !unsaved.current.has(id)) cache.current.delete(id);
    setConversations((current) => [...current.filter((item) => unsaved.current.has(item.id)), ...list]);
    const open = activeRef.current;
    if (open !== null && !kept.has(open) && !unsaved.current.has(open)) activate(null, []);
  }, [activate, guard]);

  const enter = useCallback(
    (next: Profile) => {
      setProfile(next);
      setStatus("signedIn");
      refreshAccount().catch(() => undefined);
      void loadHistory();
    },
    [loadHistory, refreshAccount],
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

  // Settings belong to this computer, not to an account, so they load before anyone signs in.
  useEffect(() => {
    api
      .getSettings()
      .then(async (stored) => {
        const saved = loadSavedLanguage();
        let chosen = stored.language;
        if (saved !== null) {
          // A choice made before settings moved to Rust, or while they could not be saved.
          if (saved !== stored.language) await api.setLanguage(saved);
          chosen = saved;
          clearSavedLanguage();
        }
        setLanguageState(chosen);
        setRetentionDays(stored.retention_days);
      })
      .catch(() => setStoreWarning(STORE_WARNING));
  }, []);

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

  /** Shows a message in its conversation and files it away. Ignored if that conversation is no longer on screen. */
  const addMessage = useCallback(
    (id: string, message: ChatMessage) => {
      if (activeRef.current !== id) return;
      const next = [...(cache.current.get(id) ?? []), message];
      cache.current.set(id, next);
      setMessages(next);
      setConversations((list) => {
        const found = list.find((item) => item.id === id);
        return found ? [{ ...found, updated_at: Date.now() }, ...list.filter((item) => item.id !== id)] : list;
      });
      if (!unsaved.current.has(id)) remember(() => api.appendMessage(id, message));
    },
    [remember],
  );

  /** Starts a conversation on the first message. If it can't be saved, it lives in memory only. */
  const startConversation = useCallback(
    async (title: string): Promise<string> => {
      let created: ConversationSummary | undefined;
      try {
        created = await guard(() => api.createConversation(title));
      } catch (error) {
        if (asFailure(error).code !== "signed_out") setStoreWarning(STORE_WARNING);
      }
      const summary = created ?? { id: crypto.randomUUID(), title, updated_at: Date.now() };
      if (!created) unsaved.current.add(summary.id);
      cache.current.set(summary.id, []);
      setConversations((list) => [summary, ...list]);
      activate(summary.id, []);
      return summary.id;
    },
    [activate, guard],
  );

  const runTurn = useCallback(
    (conversationId: string, history: ChatMessage[]) => {
      setSending(true);
      setChatError(null);
      setLive(null);
      window.clearTimeout(settle.current);
      setExpression("thinking");
      const reply = (text: string) => addMessage(conversationId, { id: crypto.randomUUID(), role: "moby", text });

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

      // Only this conversation goes to the server, never the others.
      const turn = buildTurn(history);
      guard(() => api.agentTurn(turn, language, onEvent))
        .then(() => refreshAccount().catch(() => undefined))
        .catch((failure: ApiFailure) => failTurn(failure))
        .finally(() => setSending(false));
    },
    [addMessage, failTurn, guard, language, refreshAccount, settleFace],
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
      language,
      retentionDays,
      storeWarning,
      conversations,
      activeId,
      send: (text) => {
        const trimmed = text.trim();
        if (trimmed === "" || sending || exceedsLimit(trimmed)) return;
        setSending(true);
        const message = { id: crypto.randomUUID(), role: "user" as const, text: trimmed };
        void (async () => {
          const id = activeRef.current ?? (await startConversation(titleFrom(trimmed)));
          addMessage(id, message);
          runTurn(id, [...(cache.current.get(id) ?? [])]);
        })();
      },
      retryTurn: () => {
        if (!sending && activeId !== null) runTurn(activeId, messages);
      },
      newConversation: () => {
        if (!sending) activate(null, []);
      },
      openConversation: async (id) => {
        if (sending) return;
        let loaded = cache.current.get(id);
        if (!loaded) {
          loaded = await guard(() => api.loadMessages(id));
          cache.current.set(id, loaded);
        }
        activate(id, loaded);
      },
      deleteConversation: async (id) => {
        if (sending) return;
        if (!unsaved.current.has(id)) {
          // Already gone (for example removed by retention) counts as deleted.
          await guard(() => api.deleteConversation(id)).catch((failure: ApiFailure) => {
            if (failure.code !== "not_found") throw failure;
          });
        }
        unsaved.current.delete(id);
        cache.current.delete(id);
        setConversations((list) => list.filter((item) => item.id !== id));
        if (activeRef.current === id) activate(null, []);
      },
      setLanguage: async (next) => {
        // The choice applies at once. If it can't be saved it is kept in this window's storage and tried again at the next start.
        setLanguageState(next);
        try {
          await api.setLanguage(next);
          clearSavedLanguage();
        } catch {
          saveLanguageFallback(next);
          setStoreWarning(STORE_WARNING);
        }
      },
      setRetention: async (days) => {
        const removed = await api.setRetention(days).catch((error: unknown) => {
          throw asFailure(error);
        });
        setRetentionDays(days);
        if (removed > 0 && profile) await syncHistory().catch(() => setStoreWarning(STORE_WARNING));
        return removed;
      },
      deleteAllHistory: async () => {
        await api.deleteAllHistory().catch((error: unknown) => {
          throw asFailure(error);
        });
        cache.current.clear();
        unsaved.current.clear();
        setConversations([]);
        activate(null, []);
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
    [
      activate,
      activeId,
      addMessage,
      chatError,
      conversations,
      enter,
      expression,
      guard,
      installations,
      language,
      leave,
      live,
      messages,
      profile,
      retentionDays,
      runTurn,
      sending,
      start,
      startConversation,
      status,
      storeWarning,
      syncHistory,
      usage,
    ],
  );

  return <Context.Provider value={value}>{children}</Context.Provider>;
}
