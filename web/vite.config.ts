/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

/**
 * Vite configuration for the Filebrary web UI.
 *
 * The build output in `dist/` is embedded into the server binary with
 * rust-embed. In development, `/api` and `/s/*` downloads are proxied to a
 * locally running server (override with `FILEBRARY_DEV_API`).
 */
const api = process.env.FILEBRARY_DEV_API ?? "http://127.0.0.1:8080";

export default defineConfig({
  plugins: [react()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: false,
    target: "es2022",
  },
  server: {
    proxy: {
      "/api": { target: api, changeOrigin: false },
    },
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
