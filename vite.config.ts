import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath, URL } from "node:url";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [vue(), tailwindcss()],

  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },

  build: {
    rollupOptions: {
      output: {
        manualChunks: {
          // Vue 核心
          "vendor-vue": ["vue", "vue-router", "pinia"],
          // i18n
          "vendor-i18n": ["vue-i18n"],
          // UI 组件库
          "vendor-reka": ["reka-ui"],
          // 工具库
          "vendor-utils": ["@vueuse/core", "clsx", "tailwind-merge", "class-variance-authority"],
          // 图标库
          "vendor-icons": ["lucide-vue-next", "@lucide/vue"],
          // 通知
          "vendor-sonner": ["vue-sonner"],
        },
      },
    },
  },
  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
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
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
