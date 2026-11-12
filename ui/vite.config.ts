import solid from 'vite-plugin-solid';
import { defineConfig } from 'vitest/config';

// The core listens here; the dev server passes /ws and /api through to it,
// so the interface always talks to "its own" address.
const core = process.env.AE_CORE ?? '127.0.0.1:4477';

export default defineConfig({
  plugins: [solid()],
  server: {
    port: Number(process.env.PORT) || 5180,
    proxy: {
      '/ws': { target: `ws://${core}`, ws: true },
      '/api': { target: `http://${core}` },
    },
  },
  build: { target: 'es2022', outDir: 'dist', sourcemap: true },
  // The tests so far check plain logic and need no browser.
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
    coverage: { provider: 'v8', reporter: ['text-summary', 'lcov'], include: ['src/**/*.{ts,tsx}'] },
  },
});
