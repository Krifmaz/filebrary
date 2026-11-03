import { describe, expect, it } from "vitest";
import { nextTheme } from "../../theme";
import { DEFAULT_BRANDING, brandingCss, declarations, luminance, validColour } from "./branding";

describe("branding", () => {
  it("accepts only hex colours", () => {
    expect(validColour("#fff")).toBe(true);
    expect(validColour("#A1b2C3")).toBe(true);
    for (const bad of ["fff", "#ffff", "red", "#fff;}", "url(x)", ""]) expect(validColour(bad)).toBe(false);
  });

  it("drops unknown variables and unsafe values", () => {
    expect(declarations({ "--accent": "#123456", "--evil": "#000", "--bg": "#fff;} body{" })).toBe("--accent:#123456;");
  });

  it("computes luminance", () => {
    expect(luminance("#000")).toBe(0);
    expect(luminance("#ffffff")).toBeCloseTo(1);
  });

  it("builds CSS per theme, with dark defaults for dark custom themes", () => {
    const css = brandingCss({ ...DEFAULT_BRANDING, light: { "--accent": "#336699" }, custom: { "--bg": "#101010" } }, { "--text": "#eeeeee" });
    expect(css).toContain(':root[data-theme="light"]{--accent:#336699;}');
    expect(css).toContain("@media (prefers-color-scheme: light){:root:not([data-theme]){--accent:#336699;}}");
    expect(css).not.toContain('data-theme="dark"');
    expect(css).toMatch(/:root\[data-theme="custom"\]\{[^}]*color-scheme:dark;[^}]*--bg:#101010;--text:#eeeeee;\}/);
  });

  it("cycles through custom only when one exists", () => {
    expect(nextTheme("dark", false)).toBe("system");
    expect(nextTheme("dark", true)).toBe("custom");
    expect(nextTheme("custom", true)).toBe("system");
    expect(nextTheme("system", true)).toBe("light");
  });
});
