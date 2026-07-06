import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import tauri from 'vite-plugin-tauri';
import path from 'path';

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    tauri(),
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