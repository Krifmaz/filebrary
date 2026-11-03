import { describe, expect, it } from "vitest";
import { basename, crumbs, formatSize, join, parent, validateName } from "./paths";

describe("paths", () => {
  it("joins and normalises", () => {
    expect(join("/", "a", "b/c")).toBe("/a/b/c");
    expect(join("/a/", "./b//")).toBe("/a/b");
    expect(join("")).toBe("/");
  });
  it("finds parent and basename", () => {
    expect(parent("/a/b/c")).toBe("/a/b");
    expect(parent("/a")).toBe("/");
    expect(parent("/")).toBe("/");
    expect(basename("/a/b.txt")).toBe("b.txt");
    expect(basename("/")).toBe("");
  });
  it("builds breadcrumbs", () => {
    expect(crumbs("/a/b")).toEqual([
      { name: "/", path: "/" },
      { name: "a", path: "/a" },
      { name: "b", path: "/a/b" },
    ]);
  });
  it("rejects unsafe names", () => {
    expect(validateName("ok.txt")).toBeNull();
    for (const bad of ["", " ", ".", "..", "a/b", "a\\b", "a\u0000b", "x".repeat(256)]) {
      expect(validateName(bad)).not.toBeNull();
    }
  });
  it("formats sizes", () => {
    expect(formatSize(0)).toBe("0 B");
    expect(formatSize(999)).toBe("999 B");
    expect(formatSize(1500)).toBe("1.5 kB");
    expect(formatSize(25_000_000)).toBe("25 MB");
  });
});
