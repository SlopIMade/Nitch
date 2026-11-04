import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// During development the Rust server runs on :8080; Vite proxies the API and HLS to it.
// NITCH_SERVER points it at another server instead, e.g. http://192.168.1.20:8080.
declare const process: { env: Record<string, string | undefined> };
const server = process.env.NITCH_SERVER ?? "http://localhost:8080";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      "/api": { target: server, ws: true },
      "/hls": server,
    },
  },
});
