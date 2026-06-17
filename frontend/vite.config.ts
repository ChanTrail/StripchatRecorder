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
        manualChunks(id) {
          if (["vue", "vue-router", "pinia"].some(p => id.includes(`/node_modules/${p}/`))) return "vendor-vue";
          if (id.includes("/node_modules/vue-i18n/") || id.includes("/node_modules/@intlify/")) return "vendor-i18n";
          if (id.includes("/node_modules/reka-ui/")) return "vendor-reka";
          if (["@vueuse/core", "clsx", "tailwind-merge", "class-variance-authority"].some(p => id.includes(`/node_modules/${p}/`))) return "vendor-utils";
          if (id.includes("/node_modules/@lucide/")) return "vendor-icons";
          if (id.includes("/node_modules/vue-sonner/")) return "vendor-sonner";
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
