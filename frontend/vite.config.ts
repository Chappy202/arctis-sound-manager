import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig(({ command }) => ({
  // emitCss:false in dev makes Svelte INJECT each component's scoped CSS via JS
  // instead of emitting separate virtual CSS modules. vite-plugin-svelte@7.1.2's
  // `load-compiled-css` hook fails to serve those virtual modules on the Vite 8
  // (Rolldown) dev path for eagerly-loaded components (App/AppShell), so their
  // styles silently went missing and the layout collapsed. Injected CSS sidesteps
  // that hook entirely. Production (`vite build`) keeps emitCss:true → a single
  // extracted stylesheet (the build path is unaffected by the bug).
  plugins: [svelte({ emitCss: command === "build" })],

  // Tauri expects a fixed port, fail if that port is not available
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: {
      // tell vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },

  // Env variables starting with the item of `envPrefix` will be exposed
  // in tauri's source code through `import.meta.env`.
  envPrefix: ["VITE_", "TAURI_ENV_*"],

  build: {
    // Tauri uses Chromium on Windows and WebKit on macOS and Linux
    target:
      process.env.TAURI_ENV_PLATFORM == "windows" ? "chrome105" : "safari13",
    // don't minify for debug builds
    // Note: Vite 8 uses oxc/rolldown; esbuild is deprecated. Use "oxc" (default) or false.
    minify: (process.env.TAURI_ENV_DEBUG ? false : "oxc") as "oxc" | false,
    // produce sourcemaps for debug builds
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
}));
