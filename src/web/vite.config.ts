import { defineConfig } from "vite";
import solidPlugin from "vite-plugin-solid";

export default defineConfig({
  root: "client",
  plugins: [solidPlugin()],
  build: {
    outDir: "../public",
    emptyOutDir: true,
  },
  server: {
    port: 5173,
    proxy: {
      // Proxy WebSocket connections to the Bun backend
      "/ws": {
        target: "ws://localhost:3000",
        ws: true,
      },
    },
  },
});
