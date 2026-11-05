import { readFileSync } from "node:fs";
import { EXPRESSIONS } from "@mobyuz/moby-protocol";
import { EXPRESSION_META, MobyFace } from "@mobyuz/moby-ui";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8");

describe("the face from @mobyuz/moby-ui", () => {
  it("covers every protocol expression and draws each one differently", () => {
    expect(Object.keys(EXPRESSION_META).sort()).toEqual([...EXPRESSIONS].sort());
    const drawn = EXPRESSIONS.map((expression) => renderToStaticMarkup(createElement(MobyFace, { expression })));
    expect(new Set(drawn).size).toBe(EXPRESSIONS.length);
  });

  it("keeps the accessible name the app always had", () => {
    expect(renderToStaticMarkup(createElement(MobyFace, { expression: "thinking" }))).toContain('aria-label="Moby is thinking"');
  });

  it("loads the package styles once and has no copy of the old face CSS", () => {
    const main = read("../src/main.tsx");
    expect(main).toContain('import "@mobyuz/moby-ui/styles.css"');
    expect(main).toContain('import "@mobyuz/moby-ui/mood.css"');
    const css = read("../src/styles.css") + read("../src/app.css");
    expect(css).not.toMatch(/\.robot(?![\w-])|var\(--mood\)|--mood:/);
    expect(css).toContain("var(--moby-mood)");
  });
});
