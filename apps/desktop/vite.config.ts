import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

// Bundle the local connection and worker controls. The dashboard opens separately.
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
