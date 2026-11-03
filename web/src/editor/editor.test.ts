import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";
import { loadText, SaveConflict, saveText } from "./api";
import { isTextFile, MAX_TEXT_BYTES } from "./textFiles";

afterEach(() => vi.unstubAllGlobals());

const reply = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

const file = (name: string, mime: string | null = null, size = 10) => ({ name, mime, size, is_dir: false });

describe("isTextFile", () => {
  it("accepts text by MIME type, extension or well-known name", () => {
    expect(isTextFile(file("a.bin", "text/plain"))).toBe(true);
    expect(isTextFile(file("x", "application/json"))).toBe(true);
    expect(isTextFile(file("x", "application/vnd.api+json"))).toBe(true);
    expect(isTextFile(file("main.rs"))).toBe(true);
    expect(isTextFile(file("App.TSX"))).toBe(true);
    expect(isTextFile(file("Dockerfile"))).toBe(true);
    expect(isTextFile(file(".gitignore"))).toBe(true);
  });
  it("refuses binaries, folders and large files", () => {
    expect(isTextFile(file("photo.jpg", "image/jpeg"))).toBe(false);
    expect(isTextFile(file("archive.zip"))).toBe(false);
    expect(isTextFile({ ...file("src"), is_dir: true })).toBe(false);
    expect(isTextFile(file("big.log", "text/plain", MAX_TEXT_BYTES + 1))).toBe(false);
  });
});

describe("editor api", () => {
  it("loads with an encoded path", async () => {
    const fetchMock = vi.fn().mockResolvedValue(reply(200, { path: "/a b.md", content: "hi", etag: '"e1"', size: 2, modified: null }));
    vi.stubGlobal("fetch", fetchMock);
    const f = await loadText("s", "/a b.md");
    expect(f.etag).toBe('"e1"');
    expect(fetchMock.mock.calls[0]![0]).toBe("/api/sources/s/text?path=%2Fa+b.md");
  });

  it("saves raw text with If-Match, or If-None-Match to create", async () => {
    const fetchMock = vi.fn().mockImplementation(() =>
      Promise.resolve(reply(200, { path: "/a", etag: '"e2"', size: 3, modified: null })),
    );
    vi.stubGlobal("fetch", fetchMock);
    expect((await saveText("s", "/a", "new", '"e1"')).etag).toBe('"e2"');
    let init = fetchMock.mock.calls[0]![1] as RequestInit;
    let h = new Headers(init.headers);
    expect(init.method).toBe("PUT");
    expect(init.body).toBe("new");
    expect(h.get("If-Match")).toBe('"e1"');
    expect(h.get("If-None-Match")).toBeNull();
    expect(h.get("Content-Type")).toContain("text/plain");

    await saveText("s", "/b", "", null);
    init = fetchMock.mock.calls[1]![1] as RequestInit;
    h = new Headers(init.headers);
    expect(h.get("If-None-Match")).toBe("*");
    expect(h.get("If-Match")).toBeNull();
  });

  it("reports conflicts with the server's current ETag", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(reply(412, { error: "precondition_failed", message: "changed", etag: '"e9"' })),
    );
    const err = await saveText("s", "/a", "x", '"e1"').catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SaveConflict);
    expect((err as SaveConflict).currentEtag).toBe('"e9"');
  });

  it("turns other failures into ApiError with the code", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(reply(415, { error: "not_text", message: "nope" })));
    const err = await loadText("s", "/x.bin").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).code).toBe("not_text");
    expect((err as ApiError).status).toBe(415);
  });
});
