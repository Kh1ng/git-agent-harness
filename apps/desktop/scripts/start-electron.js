#!/usr/bin/env node

import { spawn } from 'node:child_process';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));

const electronPath = process.argv[0];
const mainPath = resolve(__dirname, '../dist-electron/main.cjs');

console.log(`Starting Electron app...`);
console.log(`Electron: ${electronPath}`);
console.log(`Main: ${mainPath}`);

const electronProcess = spawn(electronPath, [mainPath], {
  stdio: 'inherit',
  cwd: resolve(__dirname, '..')
});

electronProcess.on('error', (error) => {
  console.error('Failed to start Electron:', error);
  process.exit(1);
});

electronProcess.on('exit', (code, signal) => {
  console.log(`Electron process exited with code ${code} and signal ${signal}`);
  process.exit(code || 0);
});

// Handle Ctrl+C
process.on('SIGINT', () => {
  electronProcess.kill('SIGINT');
});

process.on('SIGTERM', () => {
  electronProcess.kill('SIGTERM');
});