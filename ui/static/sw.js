/**
 * Retires the old web UI's service worker.
 *
 * Browsers that used the React UI keep its worker at /sw.js and look for a new
 * version of this file. This one takes over, deletes the old caches and
 * unregisters itself, so pages load straight from the server again. The kit's
 * own offline worker replaces it when Filebrary wires that in.
 */
self.addEventListener("install", () => self.skipWaiting());

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.map((k) => caches.delete(k))))
      .then(() => self.registration.unregister())
      .then(() => self.clients.matchAll({ type: "window" }))
      .then((clients) => clients.forEach((c) => c.navigate(c.url))),
  );
});
