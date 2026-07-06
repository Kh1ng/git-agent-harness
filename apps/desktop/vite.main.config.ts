import { defineConfig } from 'vite';
import { builtinModules } from 'module';
import path from 'path';

export default defineConfig({
  build: {
    outDir: 'dist-electron',
    lib: {
      entry: path.resolve(__dirname, 'src/main/main.ts'),
      name: 'main',
      fileName: 'main.cjs',
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
    emptyOutDir: true,
  },
});