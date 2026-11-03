import { describe, expect, it } from "vitest";
import { isGrid, loadViewMode, MAX_FOLDERS, modeForKey, nextMode, previewKind, saveViewMode } from "./viewMode";

/** In-memory `Storage` stand-in. */
function memStore(): Storage {
  const m = new Map<string, string>();
  return {
    get length() {
      return m.size;
    },
    clear: () => m.clear(),
    getItem: (k) => m.get(k) ?? null,
    key: (i) => [...m.keys()][i] ?? null,
    removeItem: (k) => void m.delete(k),
    setItem: (k, v) => void m.set(k, v),
  };
}

describe("view modes", () => {
  it("maps shortcut keys and cycles", () => {
    expect(modeForKey("1")).toBe("details");
    expect(modeForKey("5")).toBe("large");
    expect(modeForKey("6")).toBeUndefined();
    expect(nextMode("details")).toBe("compact");
    expect(nextMode("large")).toBe("details");
    expect(nextMode("details", -1)).toBe("large");
    expect(isGrid("medium")).toBe(true);
    expect(isGrid("compact")).toBe(false);
  });

  it("defaults to details", () => {
    expect(loadViewMode(1, "s", "/", memStore())).toBe("details");
    expect(loadViewMode(1, "s", "/", undefined)).toBe("details");
  });

  it("remembers per folder and uses the last choice as the default", () => {
    const st = memStore();
    saveViewMode(1, "s", "/photos", "large", st);
    expect(loadViewMode(1, "s", "/photos", st)).toBe("large");
    expect(loadViewMode(1, "s", "/other", st)).toBe("large");
    saveViewMode(1, "s", "/docs", "details", st);
    expect(loadViewMode(1, "s", "/photos", st)).toBe("large");
    expect(loadViewMode(1, "s", "/new", st)).toBe("details");
  });

  it("keeps users apart", () => {
    const st = memStore();
    saveViewMode(1, "s", "/", "small", st);
    expect(loadViewMode(2, "s", "/", st)).toBe("details");
  });

  it("ignores corrupt data and caps the folder list", () => {
    const st = memStore();
    st.setItem("filebrary.view.1", "{not json");
    expect(loadViewMode(1, "s", "/", st)).toBe("details");
    st.setItem("filebrary.view.1", JSON.stringify({ default: "huge", folders: { "s:/": "nope", "s:/a": "small" } }));
    expect(loadViewMode(1, "s", "/", st)).toBe("details");
    expect(loadViewMode(1, "s", "/a", st)).toBe("small");
    for (let i = 0; i < MAX_FOLDERS + 10; i++) saveViewMode(1, "s", `/f${i}`, "compact", st);
    const stored = JSON.parse(st.getItem("filebrary.view.1") ?? "{}") as { folders: object };
    expect(Object.keys(stored.folders)).toHaveLength(MAX_FOLDERS);
    expect(Object.keys(stored.folders)).not.toContain("s:/f0");
  });

  it("survives a storage that throws", () => {
    const st = memStore();
    st.setItem = () => {
      throw new Error("quota");
    };
    expect(() => saveViewMode(1, "s", "/", "small", st)).not.toThrow();
  });

  it("detects previewable kinds like the server", () => {
    expect(previewKind({ name: "IMG_1.JPG", is_dir: false })).toBe("image");
    expect(previewKind({ name: "a.heic", is_dir: false })).toBe("heif");
    expect(previewKind({ name: "a.cr3", is_dir: false })).toBe("raw");
    expect(previewKind({ name: "clip.mkv", is_dir: false })).toBe("video");
    expect(previewKind({ name: "r.docx", is_dir: false })).toBe("office");
    expect(previewKind({ name: "notes.txt", is_dir: false })).toBeUndefined();
    expect(previewKind({ name: ".jpg", is_dir: false })).toBeUndefined();
    expect(previewKind({ name: "pics.jpg", is_dir: true })).toBeUndefined();
  });
});
