import { defineConfig } from "vite";
import Components from "unplugin-vue-components/vite";
import { NaiveUiResolver } from "unplugin-vue-components/resolvers";
import vue from "@vitejs/plugin-vue";
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [
    vue(),
    // 只自动引入用到的 naive-ui 组件，别整包打进首屏
    Components({ resolvers: [NaiveUiResolver()], dts: false }),
  ],
  build: {
    // 把大依赖拆开：首屏只加载 vue + naive-ui，xterm 等开终端再拉。
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (id.includes("@xterm/")) return "xterm";
          if (id.includes("naive-ui")) return "naive-ui";
          if (id.includes("node_modules/vue") || id.includes("@vue/")) return "vue";
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
    host: host || "127.0.0.1",
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
