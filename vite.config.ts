import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5180,
    strictPort: false,
    host: true,
    watch: {
      ignored: ["**/src-tauri/**"]
    }
  }
});
