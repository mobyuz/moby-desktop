import { readFileSync } from "node:fs";
import { EXPRESSIONS } from "@mobyuz/moby-protocol";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { EXPRESSION_META } from "../src/face/expressions";
import { MobyFace } from "../src/face/MobyFace";

const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");

describe("virtual Moby face", () => {
  it("has metadata for every protocol expression and nothing else", () => {
    expect(Object.keys(EXPRESSION_META).sort()).toEqual([...EXPRESSIONS].sort());
  });

  it("draws each of the nine expressions differently", () => {
    const drawn = EXPRESSIONS.map((expression) => renderToStaticMarkup(createElement(MobyFace, { expression })));
    expect(new Set(drawn).size).toBe(EXPRESSIONS.length);
    EXPRESSIONS.forEach((expression, index) => {
      expect(drawn[index]).toContain(`data-expression="${expression}"`);
    });
  });

  it("animates every expression with its own motion defined in CSS", () => {
    const animations = EXPRESSIONS.map((expression) => EXPRESSION_META[expression].animation);
    expect(new Set(animations).size).toBe(EXPRESSIONS.length);
    for (const animation of animations) {
      expect(css).toContain(`[data-animation="${animation}"]`);
    }
    const keyframes = [...css.matchAll(/@keyframes ([\w-]+)/g)].map((match) => match[1]);
    for (const used of css.matchAll(/animation:\s*(?!none)([\w-]+)/g)) {
      expect(keyframes).toContain(used[1]);
    }
  });

  it("keeps the microphone LEDs consistent with mute and disconnect", () => {
    expect(EXPRESSION_META.muted.greenLed).toBe("off");
    expect(EXPRESSION_META.muted.redLed).toBe("off");
    expect(EXPRESSION_META.disconnected.greenLed).toBe("off");
    expect(EXPRESSION_META.listening.redLed).toBe("pulse");
    for (const expression of EXPRESSIONS) {
      if (expression !== "listening") expect(EXPRESSION_META[expression].redLed).toBe("off");
    }
  });
});
