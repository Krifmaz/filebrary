import { describe, expect, it } from "vitest";
import { parse } from "../../router";
import { parseSize, passwordProblem, roleLabel, togglePerm } from "./api";
import { generatePassword } from "./UsersPage";

describe("account routes", () => {
  it("parses settings tabs, users and setup", () => {
    expect(parse("/settings")).toEqual({ name: "settings", tab: "account" });
    expect(parse("/settings/security")).toEqual({ name: "settings", tab: "security" });
    expect(parse("/settings/preferences")).toEqual({ name: "settings", tab: "preferences" });
    expect(parse("/settings/appearance")).toEqual({ name: "appearance" });
    expect(parse("/settings/nope")).toEqual({ name: "notFound" });
    expect(parse("/admin/users")).toEqual({ name: "users" });
    expect(parse("/setup")).toEqual({ name: "setup" });
  });
});

describe("permissions", () => {
  it("any permission implies view", () => {
    expect(togglePerm([], "upload", true)).toEqual(["view", "upload"]);
  });
  it("removing view removes everything", () => {
    expect(togglePerm(["view", "download", "share"], "view", false)).toEqual([]);
  });
  it("keeps display order", () => {
    expect(togglePerm(["share", "view"], "download", true)).toEqual(["view", "download", "share"]);
  });
});

describe("helpers", () => {
  it("parses sizes", () => {
    expect(parseSize("")).toBeNull();
    expect(parseSize("1024")).toBe(1024);
    expect(parseSize("10G")).toBe(10 * 1024 ** 3);
    expect(parseSize("1.5 MiB")).toBe(1.5 * 1024 ** 2);
    expect(parseSize("500 mb")).toBe(500 * 1024 ** 2);
    expect(parseSize("lots")).toBeNaN();
  });
  it("checks passwords like the server", () => {
    expect(passwordProblem("short")).not.toBeNull();
    expect(passwordProblem("long enough")).toBeNull();
    expect(passwordProblem("long enough", "different")).toMatch(/match/);
  });
  it("labels roles", () => {
    expect(roleLabel("read_only")).toBe("Read-only");
    expect(roleLabel("admin")).toBe("Administrator");
  });
  it("generates readable passwords", () => {
    const p = generatePassword();
    expect(p).toHaveLength(16);
    expect(p).toMatch(/^[a-zA-Z2-9]+$/);
    expect(p).not.toMatch(/[lIO01o]/);
  });
});
