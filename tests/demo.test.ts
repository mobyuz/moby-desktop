import { describe, expect, it } from "vitest";
import { DEMO_ACCOUNT, DemoBackend, DemoError } from "../src/demo";

describe("demo backend", () => {
  it("rejects short passwords and bad emails like the real API", async () => {
    const backend = new DemoBackend();
    await expect(backend.login(DEMO_ACCOUNT.email, "short")).rejects.toMatchObject({ code: "weak_password" });
    await expect(backend.login("not-an-email", DEMO_ACCOUNT.password)).rejects.toMatchObject({ code: "invalid_email" });
  });

  it("requires consent to register and signs the demo account in", async () => {
    const backend = new DemoBackend();
    await expect(backend.register("new@moby.test", "a-long-password-123", false)).rejects.toBeInstanceOf(DemoError);
    const profile = await backend.login(DEMO_ACCOUNT.email, DEMO_ACCOUNT.password);
    expect(profile.email).toBe(DEMO_ACCOUNT.email);
    expect(await backend.listInstallations()).toHaveLength(2);
  });

  it("revokes consent and spends usage without going below zero", async () => {
    const backend = new DemoBackend();
    await backend.login(DEMO_ACCOUNT.email, DEMO_ACCOUNT.password);
    const consents = await backend.setConsent("cloud_voice", false);
    expect(consents.find((item) => item.type === "cloud_voice")?.revoked_at).not.toBeNull();
    const usage = backend.spend({ tts: 1_000_000 });
    expect(usage.tts_chars_remaining).toBe(0);
  });

  it("deletes the account only with the right password", async () => {
    const backend = new DemoBackend();
    await backend.login(DEMO_ACCOUNT.email, DEMO_ACCOUNT.password);
    await expect(backend.deleteAccount("wrong-password-123456")).rejects.toMatchObject({ code: "invalid_credentials" });
    await expect(backend.deleteAccount(DEMO_ACCOUNT.password)).resolves.toBeUndefined();
  });
});
