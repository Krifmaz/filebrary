import { afterEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, csrfToken, query } from "./api";

afterEach(() => vi.unstubAllGlobals());

const reply = (status: number, body: unknown) =>
  new Response(body === undefined ? null : JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

describe("api", () => {
  it("reads the CSRF cookie", () => {
    expect(csrfToken("a=1; filebrary_csrf=abc%3D; b=2")).toBe("abc=");
    expect(csrfToken("a=1")).toBeUndefined();
  });
  it("builds queries with repeated keys", () => {
    expect(query({ path: ["/a", "/b c"], x: undefined })).toBe("?path=%2Fa&path=%2Fb+c");
    expect(query({})).toBe("");
  });
  it("sends the session CSRF token on unsafe requests only", async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(reply(200, { user: { id: 1, username: "u", is_admin: false }, csrf_token: "T" }))
      .mockResolvedValueOnce(reply(200, { path: "/", entries: [] }))
      .mockResolvedValueOnce(new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetchMock);
    expect((await api.me()).username).toBe("u");
    await api.list("s", "/");
    await api.mkdir("s", "/x");
    const get = fetchMock.mock.calls[1]![1] as RequestInit;
    const post = fetchMock.mock.calls[2]![1] as RequestInit;
    expect(new Headers(get.headers).get("X-CSRF-Token")).toBeNull();
    expect(new Headers(post.headers).get("X-CSRF-Token")).toBe("T");
    expect(fetchMock.mock.calls[2]![0]).toBe("/api/sources/s/mkdir");
  });
  it("turns error bodies into ApiError", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(reply(401, { error: "totp_required", message: "Code needed" })));
    const err = await api.login("u", "p").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(401);
    expect((err as ApiError).code).toBe("totp_required");
    expect((err as ApiError).message).toBe("Code needed");
  });
  it("reports network failures with status 0", async () => {
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new TypeError("offline")));
    await expect(api.sources()).rejects.toMatchObject({ status: 0 });
  });
});
