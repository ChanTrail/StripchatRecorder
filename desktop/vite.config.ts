import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath, URL } from "node:url";

// https://tauri.app/start/frontend/vite/
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [vue(), tailwindcss()],

  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },

  // Tauri 要求构建产物输出到 dist/，相对于 tauri.conf.json 中配置的 frontendDist
  // Tauri requires build output to dist/, relative to frontendDist in tauri.conf.json
  build: {
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (["vue", "vue-router", "pinia"].some((m) => id.includes(`/node_modules/${m}/`))) return "vendor-vue";
          if (id.includes("/node_modules/vue-i18n/")) return "vendor-i18n";
          if (id.includes("/node_modules/reka-ui/")) return "vendor-reka";
          if (["@vueuse/core", "clsx", "tailwind-merge", "class-variance-authority"].some((m) => id.includes(`/node_modules/${m}/`))) return "vendor-utils";
          if (id.includes("/node_modules/@lucide/")) return "vendor-icons";
          if (id.includes("/node_modules/vue-sonner/")) return "vendor-sonner";
        },
      },
    },
  },

  // Tauri 开发服务器配置 / Tauri dev server config
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
  },
});
