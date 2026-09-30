import preact from "@preact/preset-vite";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [preact()],
  test: {
    // The views build real DOM nodes; happy-dom provides document in tests.
    environment: "happy-dom",
    include: ["test/**/*.test.{ts,tsx}"],
  },
});
