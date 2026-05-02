import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import checker from "vite-plugin-checker";

const host = process.env.TAURI_DEV_HOST;
const xtermRequestModeEnum =
  'let r;(P=>(P[P.NOT_RECOGNIZED=0]="NOT_RECOGNIZED",P[P.SET=1]="SET",P[P.RESET=2]="RESET",P[P.PERMANENTLY_SET=3]="PERMANENTLY_SET",P[P.PERMANENTLY_RESET=4]="PERMANENTLY_RESET"))(r||={});';
const xtermRequestModeEnumSafe =
  "const r={NOT_RECOGNIZED:0,SET:1,RESET:2,PERMANENTLY_SET:3,PERMANENTLY_RESET:4};";

export default defineConfig(async () => ({
  plugins: [
    {
      name: "patch-xterm-request-mode-enum",
      enforce: "pre",
      transform(code, id) {
        if (!id.includes("@xterm/xterm/lib/xterm.mjs")) return null;
        if (!code.includes(xtermRequestModeEnum)) return null;
        return code.replace(xtermRequestModeEnum, xtermRequestModeEnumSafe);
      },
    },
    react(),
    checker({ typescript: true }),
  ],
  clearScreen: false,
  build: {
    target: "safari13",
    chunkSizeWarningLimit: 1000,
  },
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**", "**/node_modules/**", "**/dist/**"],
      usePolling: false,
    },
  },
}));
