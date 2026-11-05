import { describe, expect, it } from "vitest";
import { parseLanguage, parseTheme } from "../src/prefs";

describe("saved preferences", () => {
  it("keeps the two explicit themes and follows the system otherwise", () => {
    expect(parseTheme("light")).toBe("light");
    expect(parseTheme("dark")).toBe("dark");
    expect(parseTheme(null)).toBe("system");
    expect(parseTheme("sepia")).toBe("system");
  });

  it("falls back to Uzbek for an unknown reply language", () => {
    expect(parseLanguage("ru")).toBe("ru");
    expect(parseLanguage("en")).toBe("en");
    expect(parseLanguage(null)).toBe("uz");
    expect(parseLanguage("de")).toBe("uz");
  });
});
