#!/usr/bin/env node

import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const args = process.argv.slice(2);
const command = args[0];

const scripts = {
  'dev': () => {
    const server = spawn('npm', ['run', 'dev'], {
      stdio: 'inherit',
      cwd: path.resolve(__dirname, '..', 'apps/server')
    });
    
    const web = spawn('npm', ['run', 'dev'], {
      stdio: 'inherit',
      cwd: path.resolve(__dirname, '..', 'apps/web')
    });
    
    ['SIGINT', 'SIGTERM'].forEach(signal => {
      process.on(signal, () => {
        server.kill(signal);
        web.kill(signal);
        process.exit(0);
      });
    });
  },
  'dev:server': () => {
    spawn('npm', ['run', 'dev'], {
      stdio: 'inherit',
      cwd: path.resolve(__dirname, '..', 'apps/server')
    });
  },
  'dev:web': () => {
    spawn('npm', ['run', 'dev'], {
      stdio: 'inherit',
      cwd: path.resolve(__dirname, '..', 'apps/web')
    });
  },
  'dev:desktop': () => {
    spawn('npm', ['run', 'start'], {
      stdio: 'inherit',
      cwd: path.resolve(__dirname, '..', 'apps/desktop')
    });
  }
};

if (scripts[command]) {
  scripts[command]();
} else {
  console.error(`Unknown command: ${command}`);
  console.log('Available commands: dev, dev:server, dev:web, dev:desktop');
  process.exit(1);
}