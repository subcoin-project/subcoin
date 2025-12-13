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
      // The target can be overridden via query param: /rpc?target=host:port
      "/rpc": {
        target: "http://localhost:9944",
        changeOrigin: true,
        configure: (proxy, _options) => {
          proxy.on("proxyReq", (proxyReq, req, _res) => {
            // Check for target override in query string
            const url = new URL(req.url || "", `http://${req.headers.host}`);
            const targetOverride = url.searchParams.get("target");
            if (targetOverride) {
              // Update the host header for the overridden target
              proxyReq.setHeader("host", targetOverride);
            }
          });
        },
        router: (req) => {
          // Check for target override in query string
          const url = new URL(req.url || "", `http://${req.headers.host}`);
          const targetOverride = url.searchParams.get("target");
          if (targetOverride) {
            return `http://${targetOverride}`;
          }
          return "http://localhost:9944";
        },
        rewrite: (path) => {
          // Remove query params from the forwarded path
          return path.replace(/^\/rpc.*/, "");
        },
      },
    },
  },
});
