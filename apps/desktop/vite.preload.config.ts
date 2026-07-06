import { defineConfig } from 'vite';
import { builtinModules } from 'module';
import path from 'path';

export default defineConfig({
  build: {
    outDir: 'dist-electron',
    lib: {
      entry: path.resolve(__dirname, 'src/main/preload.ts'),
      name: 'preload',
      fileName: 'preload.cjs',
      format: 'cjs',
    },
    rollupOptions: {
      external: [
        'electron',
        ...builtinModules,
      ],
      output: {
        entryFileNames: '[name].cjs',
      },
    },
    emptyOutDir: false, // Don't clear the directory as main.js is also there
  },
});