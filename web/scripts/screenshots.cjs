/**
 * Takes screenshots of every main screen of the web UI against a mocked API,
 * at phone, tablet and desktop width, in light and dark.
 *
 *   npm run dev -- --port 5179 &
 *   node scripts/screenshots.cjs http://localhost:5179 ./shots
 *
 * Needs Playwright with Chromium installed (it is not a dependency of the
 * web UI). Every request under /api is answered here, so no server is
 * needed. Set SHOTS=name1,name2 to take only some screens.
 */
const { chromium } = require("playwright");
const fs = require("fs");

const [, , base = "http://localhost:5179", out = "./shots"] = process.argv;
const only = process.env.SHOTS ? new Set(process.env.SHOTS.split(",")) : null;
fs.mkdirSync(out, { recursive: true });

const user = { id: 1, username: "azule", is_admin: true };
const full = { view: true, download: true, upload: true, edit: true, delete: true, share: true };
const readOnly = { view: true, download: true, upload: false, edit: false, delete: false, share: false };
const now = Date.parse("2026-11-03T20:15:00Z");
const ago = (h) => new Date(now - h * 3600e3).toISOString();
const sources = [
  { id: "home", name: "Home", permissions: full },
  { id: "nas", name: "NAS (SMB)", permissions: { ...full, delete: false } },
  { id: "photos-s3", name: "Photos (S3)", permissions: readOnly },
];
const mk = (name, is_dir, size, h, dir = "") => ({ name, path: `${dir}/${name}`, is_dir, size, modified: ago(h), mime: null });
const listings = {
  "/": [
    mk("Documents", true, 0, 3), mk("Music", true, 0, 50), mk("Photos", true, 0, 2), mk("Projects", true, 0, 26),
    mk("budget-2026.xlsx", false, 48213, 5), mk("holiday-in-lisbon.mp4", false, 734003200, 30), mk("notes.md", false, 3120, 1),
    mk("report-q3.pdf", false, 1203400, 72), mk("sunset.jpg", false, 4212000, 8), mk("backup.tar.gz", false, 2147483648, 400),
  ],
  "/Empty": [],
};
const annotations = { Photos: { starred: true, tags: [] }, "report-q3.pdf": { starred: true, tags: [{ id: 1, name: "Work", color: "#38bdf8" }] }, "budget-2026.xlsx": { starred: false, tags: [{ id: 2, name: "Finance", color: "#22c55e" }] } };
const stars = [
  { source: "home", path: "/Photos", name: "Photos", starred_at: 1 },
  { source: "home", path: "/report-q3.pdf", name: "report-q3.pdf", starred_at: 2 },
  { source: "nas", path: "/Media/Films", name: "Films", starred_at: 3 },
];
const shares = [
  { id: "a", token: "t1", url: "https://files.example.org/s/t1", source: "home", paths: ["/Photos"], expires_at: new Date(now + 6 * 864e5).toISOString(), max_downloads: null, downloads: 4, upload_only: false, has_password: true, created_at: ago(30) },
  { id: "b", token: "t2", url: "https://files.example.org/s/t2", source: "home", paths: ["/Projects/Inbox"], expires_at: null, max_downloads: null, downloads: 0, upload_only: true, has_password: false, created_at: ago(90) },
  { id: "c", token: "t3", url: "https://files.example.org/s/t3", source: "nas", paths: ["/a.pdf", "/b.pdf", "/c.pdf"], expires_at: ago(5), max_downloads: 10, downloads: 10, upload_only: false, has_password: false, created_at: ago(400) },
];
const collections = [
  { id: 1, name: "Trip to Lisbon", item_count: 3, created_at: 1, updated_at: 2 },
  { id: 2, name: "Tax 2026", item_count: 2, created_at: 1, updated_at: 2 },
];
const collection1 = {
  id: 1, name: "Trip to Lisbon",
  items: [
    { id: 1, source: "home", path: "/sunset.jpg", name: "sunset.jpg", kind: "file", size: 4212000, mime: "image/jpeg" },
    { id: 2, source: "home", path: "/holiday-in-lisbon.mp4", name: "holiday-in-lisbon.mp4", kind: "file", size: 734003200, mime: "video/mp4" },
    { id: 3, source: "nas", path: null, name: "itinerary.pdf", kind: null, size: null, mime: null },
  ],
};
const hits = [
  { source: "home", path: "/report-q3.pdf", name: "report-q3.pdf", type: "document", is_dir: false, size: 1203400, modified: ago(72), mime: "application/pdf" },
  { source: "nas", path: "/Archive/report-q2.pdf", name: "report-q2.pdf", type: "document", is_dir: false, size: 998000, modified: ago(2000), mime: "application/pdf" },
  { source: "home", path: "/Projects/Reports", name: "Reports", type: "dir", is_dir: true, size: 0, modified: ago(100), mime: null },
];

async function mock(page, { signedIn = true, emptyStars = false } = {}) {
  await page.route("**/api/**", async (r) => {
    const u = new URL(r.request().url());
    const p = u.pathname;
    const m = r.request().method();
    const j = (b, s = 200) => r.fulfill({ status: s, contentType: "application/json", body: JSON.stringify(b) });
    if (p === "/api/me") return signedIn ? j({ user, csrf_token: "x" }) : j({ error: "unauthorized" }, 401);
    if (p === "/api/branding") return j({ name: "Filebrary", default_theme: "system", light: {}, dark: {}, custom: {}, logo_url: null });
    if (p === "/api/me/theme") return j({ theme: "system", colours: {} });
    if (p === "/api/sources") return j(sources);
    if (p.endsWith("/list")) {
      const path = u.searchParams.get("path") || "/";
      return j({ path, entries: listings[path] ?? listings["/"].map((e) => ({ ...e, path: `${path}${e.path}` })) });
    }
    if (p === "/api/annotations") return j(annotations);
    if (p === "/api/stars") return j(emptyStars ? [] : stars);
    if (p === "/api/shares" && m === "POST") return j({ ...shares[0], id: "n", has_password: true, max_downloads: 5 });
    if (p === "/api/shares") return j(shares);
    if (p === "/api/collections") return j(collections);
    if (p === "/api/collections/1") return j(collection1);
    if (p === "/api/search") return j({ hits, has_more: false, indexing: false });
    if (p === "/api/previews") return j({ enabled: true, kinds: { image: true, video: true, pdf: true } });
    if (p.endsWith("/preview")) return r.fulfill({ path: `${__dirname}/../public/icon-512.png` });
    if (p.startsWith("/api/public/shares/")) return j({ name: "Holiday photos", is_dir: true, upload_only: false, needs_password: false, entries: listings["/"].slice(4) });
    if (p.endsWith("/uploads") && m === "POST") {
      const body = JSON.parse(r.request().postData() || "{}");
      return j({ id: body.path, offset: 0, size: body.size });
    }
    if (p.startsWith("/api/uploads/") && m === "PATCH") {
      const id = decodeURIComponent(p.slice("/api/uploads/".length));
      const off = Number(r.request().headers()["upload-offset"] || 0);
      if (id.includes("broken")) return j({ error: "quota", message: "Storage quota exceeded" }, 403);
      if (id.includes("big") && off > 0) return; // hang: stays in progress
      const len = (r.request().postDataBuffer() || Buffer.alloc(0)).length;
      return j({ id, offset: off + len });
    }
    return j([]);
  });
}

const VIEWPORTS = {
  desktop: { width: 1440, height: 900 },
  tablet: { width: 900, height: 1100 },
  phone: { width: 390, height: 844 },
};

/** name → [path, setup?, options] */
const SCREENS = {
  login: ["/login", null, { signedIn: false }],
  home: ["/"],
  "home-empty-stars": ["/", null, { emptyStars: true }],
  files: ["/files/home/"],
  "files-grid": ["/files/home/", async (p) => { await p.getByRole("radio", { name: "Medium icons" }).click(); }],
  "files-grid-large": ["/files/home/", async (p) => { await p.getByRole("radio", { name: "Large icons" }).click(); }],
  "files-compact": ["/files/home/", async (p) => { await p.getByRole("radio", { name: "Compact list" }).click(); }],
  "files-selected": ["/files/home/", async (p) => {
    const rows = p.locator("tbody tr[data-path]");
    await rows.nth(4).click();
    await rows.nth(7).click({ modifiers: ["Shift"] });
  }],
  "files-context-menu": ["/files/home/", async (p, vp) => {
    const row = p.locator("tbody tr[data-path]").nth(7);
    if (vp === "phone") await row.locator(".row-menu").click();
    else await row.click({ button: "right", position: { x: 260, y: 20 } });
  }],
  "files-empty": ["/files/home/Empty"],
  "files-uploading": ["/files/home/", async (p) => {
    await p.locator('input[type="file"][multiple]').setInputFiles([
      { name: "big-video.mov", mimeType: "video/quicktime", buffer: Buffer.alloc(20 * 1024 * 1024) },
      { name: "notes-final.txt", mimeType: "text/plain", buffer: Buffer.from("hello") },
      { name: "broken-scan.pdf", mimeType: "application/pdf", buffer: Buffer.alloc(1000) },
    ]);
    await p.waitForTimeout(800);
  }],
  "dialog-share": ["/files/home/", async (p) => {
    await p.locator("tbody tr[data-path]").nth(2).click({ position: { x: 600, y: 20 } }).catch(() => {});
    await p.locator("tbody tr[data-path]").nth(2).locator(".row-menu").click();
    await p.getByRole("menuitem", { name: "Share…" }).click();
  }],
  "dialog-delete": ["/files/home/", async (p) => {
    await p.locator("tbody tr[data-path]").nth(5).locator('input[type="checkbox"]').check();
    await p.locator("tbody tr[data-path]").nth(6).locator('input[type="checkbox"]').check();
    await p.keyboard.press("Delete").catch(() => {});
    if (!(await p.locator("dialog[open]").count())) await p.getByRole("button", { name: "Delete" }).first().click();
  }],
  "dialog-move": ["/files/home/", async (p) => {
    await p.locator("tbody tr[data-path]").nth(5).locator('input[type="checkbox"]').check();
    await p.locator("tbody tr[data-path]").nth(5).locator(".row-menu").click();
    await p.getByRole("menuitem", { name: "Move to…" }).click();
  }],
  drawer: ["/files/home/", async (p, vp) => { if (vp === "phone") await p.getByRole("button", { name: "Open menu" }).click(); }],
  starred: ["/starred"],
  search: ["/search?q=report"],
  shared: ["/shares"],
  collections: ["/collections", async (p) => { await p.getByRole("button", { name: /Trip to Lisbon/ }).click(); }],
  appearance: ["/settings/appearance"],
  public: ["/s/t1/"],
  "not-found": ["/nope"],
};

(async () => {
  const browser = await chromium.launch();
  const themes = (process.env.THEMES || "dark,light").split(",");
  const vps = (process.env.VIEWPORTS || "desktop,phone").split(",");
  for (const theme of themes) {
    for (const vp of vps) {
      for (const [name, [url, setup, opts]] of Object.entries(SCREENS)) {
        if (only && !only.has(name)) continue;
        const ctx = await browser.newContext({ viewport: VIEWPORTS[vp], deviceScaleFactor: Number(process.env.SCALE || 1), colorScheme: theme, hasTouch: vp === "phone" });
        const page = await ctx.newPage();
        await page.addInitScript(() => localStorage.setItem("filebrary.theme", "system"));
        await mock(page, opts);
        await page.goto(base + url);
        await page.waitForTimeout(700);
        if (setup) await setup(page, vp);
        await page.waitForTimeout(450);
        await page.screenshot({ path: `${out}/${name}-${vp}-${theme}.png` });
        await ctx.close();
      }
    }
  }
  await browser.close();
})();
