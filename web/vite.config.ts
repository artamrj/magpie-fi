import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import pkg from "./package.json" with { type: "json" };

export default defineConfig({
  plugins: [react()],
  // The page and the server ship in one image with the same version, so the page can show
  // its own without asking the server.
  define: { __APP_VERSION__: JSON.stringify(pkg.version) },
  server: { proxy: { "/api": "http://127.0.0.1:7331" } },
});
