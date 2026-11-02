import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// During development the Rust server runs on :8080; Vite proxies the API and HLS to it.
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      "/api": "http://localhost:8080",
      "/hls": "http://localhost:8080",
    },
  },
});
