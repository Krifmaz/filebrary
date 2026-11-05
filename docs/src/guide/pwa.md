# Installing the app (PWA)

The Filebrary web UI is a Progressive Web App: it can be installed like a
native app on phones, tablets and computers, with its own icon and window.
There is nothing to download from an app store.

| Device | How |
|---|---|
| Android (Chrome, Edge, Samsung Internet) | Menu → **Install app** or **Add to Home screen**. |
| iPhone and iPad (Safari) | Share → **Add to Home Screen**. |
| Desktop Chrome or Edge | The install icon in the address bar, or Menu → **Install Filebrary**. |

Installing requires HTTPS (or `localhost`). The installed app talks to the
same server and uses the same account as the browser.

The app checks for a new version in the background. When the server is
updated it says so and reloads into the new version once nothing is
uploading and no edit is unsaved.

Browsers that installed the app before version 0.3.0 still have its old
offline helper (a service worker). The first visit after updating removes it
and its cache, then reloads once.
