#!/usr/bin/env node

import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const args = process.argv.slice(2);
const command = args[0];
const commandArgs = args.slice(1);

function runPair(serverScript, serverArgs = [], webEnv = {}) {
  const children = [];
  const server = spawn('npm', ['run', serverScript, ...(serverArgs.length > 0 ? ['--', ...serverArgs] : [])], {
    stdio: 'inherit',
    cwd: path.resolve(__dirname, '..', 'apps/server')
  });
  children.push(server);

  const web = spawn('npm', ['run', 'dev'], {
    stdio: 'inherit',
    cwd: path.resolve(__dirname, '..', 'apps/web'),
    env: { ...process.env, ...webEnv }
  });
  children.push(web);

  let stopping = false;
  let requestedShutdown = false;
  let remaining = children.length;
  let exitCode = 0;
  const stop = signal => {
    if (stopping) return;
    stopping = true;
    for (const child of children) child.kill(signal);
  };
  for (const signal of ['SIGINT', 'SIGTERM']) {
    process.on(signal, () => {
      requestedShutdown = true;
      stop(signal);
    });
  }
  for (const child of children) {
    child.on('exit', code => {
      remaining -= 1;
      if (!requestedShutdown && code !== 0) exitCode = code ?? 1;
      stop('SIGTERM');
      if (remaining === 0) process.exitCode = requestedShutdown ? 0 : exitCode;
    });
  }
}

const scripts = {
  'dev': () => {
    runPair('dev');
  },
  'dev:mock': () => {
    runPair('dev:mock', ['--port', '3774', ...commandArgs], {
      VITE_PROXY_TARGET: 'http://127.0.0.1:3774',
      VITE_WS_PROXY_TARGET: 'ws://127.0.0.1:3774'
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
  console.log('Available commands: dev, dev:mock, dev:server, dev:web, dev:desktop');
  process.exit(1);
}
