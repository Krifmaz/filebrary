import { describe, expect, it } from "vitest";
import { ApiError } from "./api";
import { CHUNK_SIZE, resumeKey, UploadManager, type KeyStore, type UploadApi } from "./upload";

/** In-memory fake of the upload endpoints. */
function fakeServer(opts: { failFirstChunks?: number } = {}) {
  const sessions = new Map<string, { size: number; data: number }>();
  let n = 0;
  let failures = opts.failFirstChunks ?? 0;
  const calls: string[] = [];
  const api: UploadApi = {
    uploadStart: async (_s, _p, size) => {
      const id = `u${++n}`;
      sessions.set(id, { size, data: 0 });
      calls.push(`start ${id}`);
      return { id, offset: 0 };
    },
    uploadStatus: async (id) => {
      const s = sessions.get(id);
      if (!s) throw new ApiError(404, "gone");
      calls.push(`status ${id}`);
      return { id, offset: s.data, size: s.size };
    },
    uploadChunk: async (id, offset, chunk) => {
      const s = sessions.get(id)!;
      if (failures > 0) {
        failures--;
        throw new ApiError(503, "busy");
      }
      if (offset !== s.data) throw new ApiError(409, "offset");
      s.data += chunk.size;
      calls.push(`chunk ${id} ${offset}`);
      return { id, offset: s.data };
    },
    uploadCancel: async (id) => {
      sessions.delete(id);
    },
  };
  return { api, sessions, calls };
}

function memStore(): KeyStore & { map: Map<string, string> } {
  const map = new Map<string, string>();
  return { map, get: (k) => map.get(k) ?? null, set: (k, v) => void map.set(k, v), delete: (k) => void map.delete(k) };
}

const done = (m: UploadManager) =>
  new Promise<void>((resolve) => {
    const check = () => {
      if (m.getSnapshot().every((i) => !["queued", "uploading"].includes(i.status))) resolve();
    };
    m.subscribe(check);
    check();
  });

const file = (size: number, name = "f.bin") => new File([new Uint8Array(size)], name, { lastModified: 1 });

describe("UploadManager", () => {
  it("uploads in chunks and reports completion", async () => {
    const srv = fakeServer();
    const completed: string[] = [];
    const m = new UploadManager({ api: srv.api, store: memStore(), onComplete: (_s, p) => completed.push(p) });
    m.add("src", "/dir", [{ file: file(CHUNK_SIZE * 2 + 5), relativePath: "sub/f.bin" }]);
    await done(m);
    const [item] = m.getSnapshot();
    expect(item?.status).toBe("done");
    expect(item?.path).toBe("/dir/sub/f.bin");
    expect(srv.calls.filter((c) => c.startsWith("chunk"))).toHaveLength(3);
    expect(completed).toEqual(["/dir/sub/f.bin"]);
  });

  it("retries transient failures", async () => {
    const srv = fakeServer({ failFirstChunks: 2 });
    const m = new UploadManager({ api: srv.api, store: memStore(), backoffMs: 1 });
    m.add("src", "/", [{ file: file(10), relativePath: "a" }]);
    await done(m);
    expect(m.getSnapshot()[0]?.status).toBe("done");
  });

  it("resumes a saved session from the server's offset", async () => {
    const srv = fakeServer();
    const store = memStore();
    const f = file(CHUNK_SIZE + 10);
    srv.sessions.set("old", { size: f.size, data: CHUNK_SIZE });
    store.set(resumeKey("src", "/a", f), "old");
    const m = new UploadManager({ api: srv.api, store });
    m.add("src", "/", [{ file: f, relativePath: "a" }]);
    await done(m);
    expect(srv.calls).toEqual(["status old", `chunk old ${CHUNK_SIZE}`]);
    expect(store.map.size).toBe(0);
  });

  it("starts fresh when the saved session expired", async () => {
    const srv = fakeServer();
    const store = memStore();
    const f = file(3);
    store.set(resumeKey("src", "/a", f), "missing");
    const m = new UploadManager({ api: srv.api, store });
    m.add("src", "/", [{ file: f, relativePath: "a" }]);
    await done(m);
    expect(srv.calls[0]).toBe("start u1");
    expect(m.getSnapshot()[0]?.status).toBe("done");
  });

  it("fails on client errors without retrying", async () => {
    const srv = fakeServer();
    srv.api.uploadStart = async () => {
      throw new ApiError(403, "no upload permission");
    };
    const m = new UploadManager({ api: srv.api, store: memStore() });
    m.add("src", "/", [{ file: file(3), relativePath: "a" }]);
    await done(m);
    expect(m.getSnapshot()[0]).toMatchObject({ status: "failed", error: "no upload permission" });
  });
});
