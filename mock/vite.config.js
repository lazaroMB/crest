import { defineConfig } from "vite";
import { mockApi } from "./server/api.js";

export default defineConfig({
  plugins: [mockApi()],
  server: { port: 5199 },
  css: { preprocessorOptions: { scss: { api: "modern-compiler" } } },
});
