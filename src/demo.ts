import type { ConsentRecord, ConsentKind, Installation, Profile, UsageInfo } from "./types";

/**
 * In-memory stand-in for moby-api. It enforces the same input rules as the real
 * API (for example the 15 character minimum password) and has no network access.
 */

export const DEMO_ACCOUNT = { email: "demo@moby.test", password: "demo-password-123456" } as const;

export const DAILY_LIMITS = { llm: 20_000, stt: 300, tts: 5_000 } as const;

export const CONSENT_VERSIONS: Record<ConsentKind, string> = {
  terms: "2026-11-01",
  privacy_policy: "2026-11-01",
  cloud_voice: "2026-11-01",
};

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const now = () => new Date().toISOString();
const id = () => crypto.randomUUID();

function nextUtcMidnight(): string {
  const date = new Date();
  date.setUTCHours(24, 0, 0, 0);
  return date.toISOString();
}

export class DemoError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

export class DemoBackend {
  private usage: UsageInfo = {
    llm_tokens_remaining: 17_420,
    stt_seconds_remaining: 262,
    tts_chars_remaining: 4_180,
    reset_at: nextUtcMidnight(),
  };

  private installations: Installation[] = [];
  private consents: ConsentRecord[] = [];
  private profile: Profile | null = null;

  async register(email: string, password: string, acceptedTerms: boolean): Promise<Profile> {
    await wait(350);
    this.validateCredentials(email, password);
    if (!acceptedTerms) throw new DemoError("consent_required", "Accept the terms and privacy policy to continue.");
    return this.start(email);
  }

  async login(email: string, password: string): Promise<Profile> {
    await wait(350);
    this.validateCredentials(email, password);
    if (email.trim().toLowerCase() !== DEMO_ACCOUNT.email || password !== DEMO_ACCOUNT.password) {
      throw new DemoError("invalid_credentials", "Email or password is incorrect.");
    }
    return this.start(email);
  }

  async logout(): Promise<void> {
    await wait(120);
    this.profile = null;
  }

  async getUsage(): Promise<UsageInfo> {
    await wait(80);
    return { ...this.usage };
  }

  spend(spent: Partial<{ llm: number; stt: number; tts: number }>): UsageInfo {
    this.usage = {
      ...this.usage,
      llm_tokens_remaining: Math.max(0, this.usage.llm_tokens_remaining - (spent.llm ?? 0)),
      stt_seconds_remaining: Math.max(0, this.usage.stt_seconds_remaining - (spent.stt ?? 0)),
      tts_chars_remaining: Math.max(0, this.usage.tts_chars_remaining - (spent.tts ?? 0)),
    };
    return { ...this.usage };
  }

  async listInstallations(): Promise<Installation[]> {
    await wait(80);
    return this.installations.map((item) => ({ ...item }));
  }

  async revokeInstallation(installationId: string): Promise<Installation[]> {
    await wait(200);
    this.installations = this.installations.map((item) =>
      item.id === installationId ? { ...item, revoked: true } : item,
    );
    return this.listInstallations();
  }

  async listConsents(): Promise<ConsentRecord[]> {
    await wait(80);
    return this.consents.map((item) => ({ ...item }));
  }

  async setConsent(type: ConsentKind, granted: boolean): Promise<ConsentRecord[]> {
    await wait(150);
    const stamp = now();
    if (granted) {
      this.consents = [
        ...this.consents.filter((item) => item.type !== type || item.revoked_at !== null),
        { type, version: CONSENT_VERSIONS[type], accepted_at: stamp, revoked_at: null },
      ];
    } else {
      this.consents = this.consents.map((item) =>
        item.type === type && item.revoked_at === null ? { ...item, revoked_at: stamp } : item,
      );
    }
    return this.listConsents();
  }

  async updateProfile(displayName: string): Promise<Profile> {
    await wait(200);
    if (!this.profile) throw new DemoError("unauthorized", "Sign in first.");
    const trimmed = displayName.trim();
    this.profile = { ...this.profile, display_name: trimmed === "" ? null : trimmed };
    return { ...this.profile };
  }

  async deleteAccount(password: string): Promise<void> {
    await wait(400);
    if (password !== DEMO_ACCOUNT.password) throw new DemoError("invalid_credentials", "Password is incorrect.");
    this.profile = null;
  }

  exportSize(): number {
    return JSON.stringify({ profile: this.profile, installations: this.installations, consents: this.consents }).length;
  }

  private validateCredentials(email: string, password: string): void {
    if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email.trim())) throw new DemoError("invalid_email", "Enter a valid email address.");
    if (password.length < 15) throw new DemoError("weak_password", "Password must be at least 15 characters.");
    if (password.length > 128) throw new DemoError("weak_password", "Password must be at most 128 characters.");
  }

  private start(email: string): Profile {
    const stamp = now();
    const installationId = id();
    this.profile = {
      id: id(),
      email: email.trim().toLowerCase(),
      display_name: email.trim().toLowerCase() === DEMO_ACCOUNT.email ? "Demo user" : null,
      installation_id: installationId,
    };
    this.installations = [
      { id: installationId, name: "This computer", os: "Windows 11", app_version: "0.1.0", revoked: false },
      { id: id(), name: "Work laptop", os: "Windows 11", app_version: "0.0.9", revoked: false },
    ];
    this.consents = (["terms", "privacy_policy", "cloud_voice"] as const).map((type) => ({
      type,
      version: CONSENT_VERSIONS[type],
      accepted_at: stamp,
      revoked_at: null,
    }));
    return { ...this.profile };
  }
}
