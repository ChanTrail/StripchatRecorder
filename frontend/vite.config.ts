import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath, URL } from "node:url";

// https://vite.dev/config/
export default defineConfig({
  plugins: [vue(), tailwindcss()],

  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },

  build: {
    // 输出到 build_tmp/frontend/dist/，backend 的 RustEmbed 从该目录读取
    // Output to build_tmp/frontend/dist/; backend RustEmbed reads from this directory
    outDir: "../build_tmp/frontend/dist",
    emptyOutDir: true,
    rollupOptions: {
      output: {
        manualChunks: {
          "vendor-vue": ["vue", "vue-router", "pinia"],
          "vendor-i18n": ["vue-i18n"],
          "vendor-reka": ["reka-ui"],
          "vendor-utils": ["@vueuse/core", "clsx", "tailwind-merge", "class-variance-authority"],
          "vendor-icons": ["lucide-vue-next", "@lucide/vue"],
          "vendor-sonner": ["vue-sonner"],
        },
      },
    },
  },

  server: {
    port: 5173,
    strictPort: true,
    watch: {
      ignored: ["**/backend/**", "**/build/**"],
    },
  },
});
