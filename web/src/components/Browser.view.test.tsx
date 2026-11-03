import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, type Entry, type Source } from "../api";
import { resetPreviewCapabilities } from "../features/view/previews";
import type { UploadManager } from "../upload";
import { Browser } from "./Browser";

const source: Source = {
  id: "home",
  name: "Home",
  permissions: { view: true, download: true, upload: true, edit: true, delete: true, share: true },
};
const entry = (name: string, is_dir = false): Entry => ({ name, path: `/${name}`, is_dir, size: 1000, modified: "2026-11-01T10:00:00Z" });
const entries = [entry("Docs", true), entry("cat.jpg"), entry("notes.txt"), entry("clip.mp4")];
const uploads = { add: vi.fn() } as unknown as UploadManager;

beforeEach(() => {
  localStorage.clear();
  resetPreviewCapabilities();
  vi.spyOn(api, "list").mockResolvedValue({ path: "/", entries });
  vi.spyOn(api, "previewCapabilities").mockResolvedValue({ enabled: true, kinds: { image: true, video: false } });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

async function mount(user = 7) {
  const r = render(<Browser source={source} path="/" uploads={uploads} refreshKey={0} user={user} />);
  await screen.findByText("cat.jpg");
  return r;
}

describe("Browser view modes", () => {
  it("starts in details with sortable columns", async () => {
    await mount();
    expect(screen.getByRole("table")).toBeTruthy();
    expect(screen.getByRole("radio", { name: "Details" }).getAttribute("aria-checked")).toBe("true");
  });

  it("switches to a grid with thumbnails for supported kinds only", async () => {
    const { container } = await mount();
    fireEvent.click(screen.getByRole("radio", { name: "Large icons" }));
    await act(async () => {});
    expect(screen.queryByRole("table")).toBeNull();
    expect(container.querySelector(".file-grid.large")).toBeTruthy();
    const imgs = [...container.querySelectorAll("img")].map((i) => i.getAttribute("src"));
    expect(imgs).toHaveLength(1);
    expect(imgs[0]).toContain("/api/sources/home/preview?path=%2Fcat.jpg&variant=thumb");
  });

  it("falls back to an icon when a thumbnail fails", async () => {
    const { container } = await mount();
    fireEvent.click(screen.getByRole("radio", { name: "Medium icons" }));
    await act(async () => {});
    const img = container.querySelector("img");
    expect(img).toBeTruthy();
    fireEvent.error(img!);
    expect(container.querySelector("img")).toBeNull();
  });

  it("changes mode with number keys and V, and remembers it per user", async () => {
    const { container, unmount } = await mount();
    const section = container.querySelector("section")!;
    fireEvent.keyDown(section, { key: "2" });
    expect(container.querySelector(".file-compact")).toBeTruthy();
    fireEvent.keyDown(section, { key: "v" });
    expect(container.querySelector(".file-grid.small")).toBeTruthy();
    fireEvent.keyDown(section, { key: "V", shiftKey: true });
    expect(container.querySelector(".file-compact")).toBeTruthy();
    unmount();
    await mount();
    expect(screen.getByRole("radio", { name: "Compact list" }).getAttribute("aria-checked")).toBe("true");
    cleanup();
    await mount(8);
    expect(screen.getByRole("table")).toBeTruthy();
  });

  it("ignores shortcuts typed into the filter", async () => {
    await mount();
    fireEvent.keyDown(screen.getByRole("searchbox"), { key: "3" });
    expect(screen.getByRole("table")).toBeTruthy();
  });

  it("moves the selection with arrow keys", async () => {
    const { container } = await mount();
    const section = container.querySelector("section")!;
    fireEvent.keyDown(section, { key: "3" });
    fireEvent.keyDown(section, { key: "ArrowRight" });
    fireEvent.keyDown(section, { key: "ArrowRight" });
    const selected = [...container.querySelectorAll('li[aria-selected="true"]')].map((l) => l.getAttribute("data-path"));
    expect(selected).toEqual(["/cat.jpg"]);
  });

  it("sorts grids with the sort control", async () => {
    const { container } = await mount();
    fireEvent.click(screen.getByRole("radio", { name: "Small icons" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Sort by" }), { target: { value: "name" } });
    fireEvent.click(screen.getByRole("button", { name: "Ascending" }));
    const order = [...container.querySelectorAll("li[data-path]")].map((l) => l.getAttribute("data-path"));
    expect(order).toEqual(["/Docs", "/notes.txt", "/clip.mp4", "/cat.jpg"]);
  });
});
