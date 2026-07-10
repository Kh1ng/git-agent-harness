import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';
import { execSync } from 'child_process';
import { readFileSync } from 'fs';

const pkgVersion = JSON.parse(readFileSync(path.resolve(__dirname, 'package.json'), 'utf-8')).version;
let commitSha = 'unknown';
try {
  commitSha = execSync('git rev-parse --short HEAD', { cwd: __dirname }).toString().trim();
} catch {
  // Not a git checkout (e.g. a tarball build) -- keep the 'unknown' fallback.
}

export default defineConfig({
  define: {
    __GAH_VERSION__: JSON.stringify(pkgVersion),
    __GAH_COMMIT__: JSON.stringify(commitSha),
  },
  plugins: [
    react(),
  ],
  resolve: {
    alias: {
      '@git-agent-harness/contracts': path.resolve(__dirname, '../../packages/contracts/src'),
      '@git-agent-harness/shared': path.resolve(__dirname, '../../packages/shared/src'),
    },
  },
  server: {
    port: 3000,
    proxy: {
      '/api': {
        target: 'http://localhost:3773',
      },
      '/ws': {
        target: 'ws://localhost:3773',
        ws: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    sourcemap: true,
  },
});
