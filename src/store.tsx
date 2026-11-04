import type { Expression } from "@mobyuz/moby-protocol";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { DemoBackend, DemoError, DEMO_ACCOUNT } from "./demo";
import type {
  Approval,
  ChatMessage,
  ConsentKind,
  ConsentRecord,
  Device,
  Installation,
  LogLine,
  Profile,
  Project,
  SpeechLanguage,
  ToolName,
  UsageInfo,
} from "./types";

export interface Settings {
  speechLanguage: SpeechLanguage;
  speakReplies: boolean;
  wakeSensitivity: number;
  autoDeleteDays: number;
}

type Step = readonly [Expression, number];

const stamp = () => new Date().toISOString();
const uid = () => crypto.randomUUID();

const INITIAL_DEVICE: Device = {
  id: "moby-sim-0001",
  name: "Moby Simulator",
  firmware: "0.2.0-sim",
  protocol: 1,
  connected: true,
  hardwareMuted: false,
  pairedAt: stamp(),
};

const INITIAL_PROJECTS: Project[] = [
  { id: "moby-api", name: "moby-api", path: "D:\\repos\\moby\\moby-api" },
  { id: "moby-desktop", name: "moby-desktop", path: "D:\\repos\\moby\\moby-desktop" },
];

const INITIAL_TOOLS: Record<ToolName, boolean> = {
  git_status: true,
  open_app: true,
  open_project: true,
  search_files: false,
  create_reminder: true,
};

const GREETING: ChatMessage = {
  id: "welcome",
  role: "moby",
  text: "Hi, I am Moby. Ask me for the git status of a project, or to open an app or set a reminder.",
  at: stamp(),
};

export interface AppState {
  backend: DemoBackend;
  profile: Profile | null;
  offline: boolean;
  usage: UsageInfo | null;
  installations: Installation[];
  consents: ConsentRecord[];
  devices: Device[];
  tools: Record<ToolName, boolean>;
  projects: Project[];
  approvals: Approval[];
  messages: ChatMessage[];
  log: LogLine[];
  settings: Settings;
  busy: boolean;
  expression: Expression;
  preview: Expression | null;
  authError: string | null;
  authBusy: boolean;
  signIn(email: string, password: string): Promise<void>;
  register(email: string, password: string, acceptedTerms: boolean): Promise<void>;
  signOut(): Promise<void>;
  continueOffline(): void;
  leaveOffline(): void;
  send(text: string): Promise<void>;
  resolveApproval(id: string, approve: boolean): void;
  toggleTool(tool: ToolName): void;
  addProject(name: string, path: string): void;
  removeProject(id: string): void;
  toggleHardwareMute(id: string): void;
  toggleConnection(id: string): void;
  addDevice(name: string): void;
  removeDevice(id: string): void;
  saveProfileName(name: string): Promise<void>;
  revokeInstallation(id: string): Promise<void>;
  setConsent(kind: ConsentKind, granted: boolean): Promise<void>;
  clearHistory(): void;
  deleteAccount(password: string): Promise<string | null>;
  updateSettings(patch: Partial<Settings>): void;
  setPreview(expression: Expression | null): void;
}

const Context = createContext<AppState | null>(null);

export function useApp(): AppState {
  const value = useContext(Context);
  if (!value) throw new Error("useApp must be used inside AppProvider");
  return value;
}

export function AppProvider({ children }: { children: ReactNode }) {
  const backend = useRef(new DemoBackend()).current;
  const [profile, setProfile] = useState<Profile | null>(null);
  const [offline, setOffline] = useState(false);
  const [usage, setUsage] = useState<UsageInfo | null>(null);
  const [installations, setInstallations] = useState<Installation[]>([]);
  const [consents, setConsents] = useState<ConsentRecord[]>([]);
  const [devices, setDevices] = useState<Device[]>([INITIAL_DEVICE]);
  const [tools, setTools] = useState(INITIAL_TOOLS);
  const [projects, setProjects] = useState(INITIAL_PROJECTS);
  const [approvals, setApprovals] = useState<Approval[]>([]);
  const [messages, setMessages] = useState<ChatMessage[]>([GREETING]);
  const [log, setLog] = useState<LogLine[]>([{ at: stamp(), level: "info", text: "App started in demo mode" }]);
  const [settings, setSettings] = useState<Settings>({
    speechLanguage: "uz",
    speakReplies: true,
    wakeSensitivity: 60,
    autoDeleteDays: 30,
  });
  const [busy, setBusy] = useState(false);
  const [flow, setFlow] = useState<Expression | null>(null);
  const [preview, setPreview] = useState<Expression | null>(null);
  const [authError, setAuthError] = useState<string | null>(null);
  const [authBusy, setAuthBusy] = useState(false);
  const flowRun = useRef(0);
  const timers = useRef<number[]>([]);

  const note = useCallback((level: LogLine["level"], text: string) => {
    setLog((lines) => [...lines.slice(-79), { at: stamp(), level, text }]);
  }, []);

  const clearTimers = () => {
    timers.current.forEach((handle) => window.clearTimeout(handle));
    timers.current = [];
  };

  useEffect(() => clearTimers, []);

  const play = useCallback((steps: readonly Step[]) => {
    clearTimers();
    const run = ++flowRun.current;
    let elapsed = 0;
    steps.forEach(([expression, duration]) => {
      timers.current.push(
        window.setTimeout(() => {
          if (flowRun.current === run) setFlow(expression);
        }, elapsed),
      );
      elapsed += duration;
    });
    timers.current.push(
      window.setTimeout(() => {
        if (flowRun.current === run) setFlow(null);
      }, elapsed),
    );
  }, []);

  const hold = useCallback((expression: Expression) => {
    clearTimers();
    flowRun.current += 1;
    setFlow(expression);
  }, []);

  const refreshAccount = useCallback(async () => {
    const [nextUsage, nextInstallations, nextConsents] = await Promise.all([
      backend.getUsage(),
      backend.listInstallations(),
      backend.listConsents(),
    ]);
    setUsage(nextUsage);
    setInstallations(nextInstallations);
    setConsents(nextConsents);
  }, [backend]);

  const enter = useCallback(
    async (next: Profile) => {
      setProfile(next);
      setOffline(false);
      await refreshAccount();
      note("info", `Signed in as ${next.email}`);
    },
    [note, refreshAccount],
  );

  const guardAuth = useCallback(async (action: () => Promise<Profile>) => {
    setAuthBusy(true);
    setAuthError(null);
    try {
      await enter(await action());
    } catch (error) {
      setAuthError(error instanceof DemoError ? error.message : "Something went wrong. Try again.");
      note("warn", "Sign-in failed");
    } finally {
      setAuthBusy(false);
    }
  }, [enter, note]);

  const voiceAllowed = (list: ConsentRecord[]) =>
    list.some((item) => item.type === "cloud_voice" && item.revoked_at === null);

  const resolveApproval = useCallback(
    (approvalId: string, approve: boolean) => {
      setApprovals((list) =>
        list.map((item) => (item.id === approvalId ? { ...item, status: approve ? "approved" : "denied" } : item)),
      );
      const target = approvals.find((item) => item.id === approvalId);
      if (!target) return;
      const say = (text: string) =>
        setMessages((list) => [...list, { id: uid(), role: "moby", text, at: stamp() }]);
      if (approve) {
        note("info", `Approved ${target.tool}`);
        say(RESULTS[target.tool]);
        play([
          ["speaking", 1800],
          ["happy", 1400],
        ]);
      } else {
        note("info", `Denied ${target.tool}`);
        say("Understood. I did not run it.");
        play([["confused", 1600]]);
      }
      setBusy(false);
    },
    [approvals, note, play],
  );

  const send = useCallback(
    async (text: string) => {
      const trimmed = text.trim();
      if (trimmed === "" || busy) return;
      setMessages((list) => [...list, { id: uid(), role: "user", text: trimmed, at: stamp() }]);
      setBusy(true);
      hold("thinking");
      await new Promise((resolve) => setTimeout(resolve, 900));
      const tool = pickTool(trimmed);
      const reply = (message: string) =>
        setMessages((list) => [...list, { id: uid(), role: "moby", text: message, at: stamp() }]);

      if (tool) {
        const draft = DRAFTS[tool](projects[0]?.name ?? "project");
        if (!tools[tool]) {
          setApprovals((list) => [{ id: uid(), tool, ...draft, status: "blocked", at: stamp() }, ...list]);
          reply(`The ${tool} tool is turned off in Tools & projects, so I did not ask.`);
          note("warn", `Blocked ${tool}: tool is off`);
          play([["confused", 1600]]);
          setBusy(false);
          return;
        }
        setApprovals((list) => [{ id: uid(), tool, ...draft, status: "pending", at: stamp() }, ...list]);
        reply(`I would like to ${draft.title.toLowerCase()}. Please approve it in Approvals or below.`);
        note("info", `Tool request: ${tool}`);
        backend.spend({ llm: 220 });
        setUsage(await backend.getUsage());
        hold("thinking");
        return;
      }

      const answer = ANSWERS[Math.abs(hash(trimmed)) % ANSWERS.length] ?? ANSWERS[0] ?? "";
      reply(answer);
      const speak = settings.speakReplies && voiceAllowed(consents) && profile !== null;
      backend.spend({ llm: 140, tts: speak ? answer.length : 0 });
      setUsage(await backend.getUsage());
      note("info", speak ? "Reply spoken with cloud voice" : "Reply shown as text");
      play(speak ? [["speaking", 2200], ["happy", 1200]] : [["happy", 1400]]);
      setBusy(false);
    },
    [backend, busy, consents, hold, note, play, profile, projects, settings.speakReplies, tools],
  );

  const value = useMemo<AppState>(() => {
    const robot = devices[0];
    const derived: Expression =
      robot?.hardwareMuted ? "muted" : devices.some((item) => !item.connected) ? "disconnected" : "idle";
    return {
      backend,
      profile,
      offline,
      usage,
      installations,
      consents,
      devices,
      tools,
      projects,
      approvals,
      messages,
      log,
      settings,
      busy,
      expression: preview ?? flow ?? derived,
      preview,
      authError,
      authBusy,
      signIn: (email, password) => guardAuth(() => backend.login(email, password)),
      register: (email, password, accepted) => guardAuth(() => backend.register(email, password, accepted)),
      signOut: async () => {
        await backend.logout();
        setProfile(null);
        setUsage(null);
        setInstallations([]);
        setConsents([]);
        note("info", "Signed out");
      },
      continueOffline: () => {
        setOffline(true);
        note("info", "Continuing without an account");
      },
      leaveOffline: () => setOffline(false),
      send,
      resolveApproval,
      toggleTool: (tool) => setTools((current) => ({ ...current, [tool]: !current[tool] })),
      addProject: (name, path) =>
        setProjects((list) => [...list, { id: `${name}-${list.length + 1}`.toLowerCase().replace(/\s+/g, "-"), name, path }]),
      removeProject: (projectId) => setProjects((list) => list.filter((item) => item.id !== projectId)),
      toggleHardwareMute: (deviceId) => {
        setDevices((list) => list.map((item) => (item.id === deviceId ? { ...item, hardwareMuted: !item.hardwareMuted } : item)));
        note("info", "Hardware mute switch changed");
      },
      toggleConnection: (deviceId) => {
        setDevices((list) => list.map((item) => (item.id === deviceId ? { ...item, connected: !item.connected } : item)));
        note("warn", "Simulated USB connection changed");
      },
      addDevice: (name) => {
        setDevices((list) => [
          ...list,
          { id: `moby-sim-${String(list.length + 1).padStart(4, "0")}`, name, firmware: "0.2.0-sim", protocol: 1, connected: true, hardwareMuted: false, pairedAt: stamp() },
        ]);
        note("info", `Paired ${name}`);
      },
      removeDevice: (deviceId) => {
        setDevices((list) => list.filter((item) => item.id !== deviceId));
        note("info", "Unpaired a robot");
      },
      saveProfileName: async (name) => {
        const next = await backend.updateProfile(name);
        setProfile(next);
      },
      revokeInstallation: async (installationId) => {
        setInstallations(await backend.revokeInstallation(installationId));
      },
      setConsent: async (kind, granted) => {
        setConsents(await backend.setConsent(kind, granted));
        note("info", `${kind} consent ${granted ? "granted" : "revoked"}`);
      },
      clearHistory: () => {
        setMessages([GREETING]);
        setApprovals([]);
        note("info", "Local history deleted");
      },
      deleteAccount: async (password) => {
        try {
          await backend.deleteAccount(password);
        } catch (error) {
          return error instanceof DemoError ? error.message : "Could not delete the account.";
        }
        setProfile(null);
        setUsage(null);
        setInstallations([]);
        setConsents([]);
        setMessages([GREETING]);
        setApprovals([]);
        note("info", "Account deleted");
        return null;
      },
      updateSettings: (patch) => setSettings((current) => ({ ...current, ...patch })),
      setPreview,
    };
  }, [approvals, authBusy, authError, backend, busy, consents, devices, flow, guardAuth, installations, log, messages, note, offline, preview, profile, projects, resolveApproval, send, settings, tools, usage]);

  return <Context.Provider value={value}>{children}</Context.Provider>;
}

export { DEMO_ACCOUNT };

function hash(text: string): number {
  let value = 0;
  for (const char of text) value = (value * 31 + char.charCodeAt(0)) | 0;
  return value;
}

function pickTool(text: string): ToolName | null {
  const lower = text.toLowerCase();
  if (/\bgit\b|status/.test(lower)) return "git_status";
  if (/remind/.test(lower)) return "create_reminder";
  if (/search|find file/.test(lower)) return "search_files";
  if (/open project/.test(lower)) return "open_project";
  if (/\bopen\b|launch/.test(lower)) return "open_app";
  return null;
}

const DRAFTS: Record<ToolName, (project: string) => Pick<Approval, "title" | "detail">> = {
  git_status: (project) => ({ title: `Read the git status of ${project}`, detail: `Read-only. Runs "git status" in the registered project ${project}.` }),
  open_app: () => ({ title: "Open Visual Studio Code", detail: "Launches the allow-listed app Visual Studio Code." }),
  open_project: (project) => ({ title: `Open ${project} in your editor`, detail: `Opens the registered project folder ${project}.` }),
  search_files: (project) => ({ title: `Search files in ${project}`, detail: "Read-only search inside an approved root with a bounded query." }),
  create_reminder: () => ({ title: "Create a reminder for 10:00", detail: "Saves a local reminder: \"Team stand-up\"." }),
};

const RESULTS: Record<ToolName, string> = {
  git_status: "On branch main. Your branch is up to date with origin/main. Nothing to commit, working tree clean. (demo result)",
  open_app: "Visual Studio Code is open. (demo result)",
  open_project: "The project is open in your editor. (demo result)",
  search_files: "Found 3 matching files. (demo result)",
  create_reminder: "Reminder saved for 10:00: Team stand-up. (demo result)",
};

const ANSWERS = [
  "Happy to help. In the full app this answer would come from the Moby backend.",
  "Got it. This is a demo reply, so I am not calling a real model yet.",
  "Sure. Once the backend is connected I will answer with a real model.",
];
