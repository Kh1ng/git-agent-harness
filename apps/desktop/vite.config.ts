import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

// Builds the local fallback page served when the central node is
// unreachable (the dashboard itself loads from the node over Tailscale —
// see tauri.conf.json's window url). `tauri build` bundles whatever lands
// in dist/ as frontendDist.
export default defineConfig({
  plugins: [
    react(),
  ],
  resolve: {
    alias: {
      '@git-agent-harness/contracts': path.resolve(__dirname, '../../packages/contracts/src'),
      '@git-agent-harness/shared': path.resolve(__dirname, '../../packages/shared/src'),
    },
  },
  build: {
    outDir: 'dist',
    sourcemap: true,
  },
});
