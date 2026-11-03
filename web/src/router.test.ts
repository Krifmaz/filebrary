import { describe, expect, it } from "vitest";
import { encodePath, filesUrl, parse } from "./router";

describe("router", () => {
  it("parses routes", () => {
    expect(parse("/")).toEqual({ name: "home" });
    expect(parse("/login", "?next=%2Ffiles%2Fa")).toEqual({ name: "login", next: "/files/a" });
    expect(parse("/shares")).toEqual({ name: "shares" });
    expect(parse("/starred")).toEqual({ name: "starred" });
    expect(parse("/search", "?q=cat&type=image")).toEqual({ name: "search", q: "cat", type: "image" });
    expect(parse("/search")).toEqual({ name: "search", q: "", type: "" });
    expect(parse("/files/local")).toEqual({ name: "files", source: "local", path: "/" });
    expect(parse("/files/local/a%20b/c")).toEqual({ name: "files", source: "local", path: "/a b/c" });
    expect(parse("/s/tok/sub")).toEqual({ name: "public", token: "tok", path: "/sub" });
    expect(parse("/nope")).toEqual({ name: "notFound" });
  });
  it("round-trips odd names", () => {
    const path = "/100% done/#1?.txt";
    expect(parse(filesUrl("src", path))).toEqual({ name: "files", source: "src", path });
    expect(encodePath("/a/b c")).toBe("a/b%20c");
  });
});
