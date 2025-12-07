import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "path";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": resolve(__dirname, "./src"),
    },
  },
  server: {
    port: 3002,
    strictPort: true,
    open: false,
    proxy: {
      // Proxy RPC requests to Subcoin node to avoid CORS issues
      "/rpc": {
        target: "http://localhost:9944",
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/rpc/, ""),
      },
    },
  },
});
