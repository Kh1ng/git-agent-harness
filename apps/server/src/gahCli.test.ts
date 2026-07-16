import { EventEmitter } from 'node:events';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const spawnMock = vi.fn();
const spawnSyncMock = vi.fn();

vi.mock('node:child_process', async () => {
  const actual = await vi.importActual<typeof import('node:child_process')>('node:child_process');
  return {
    ...actual,
    spawn: spawnMock,
    spawnSync: spawnSyncMock
  };
});

const { runConfigSet, runProfileSet, stopLoop } = await import('./gahCli.js');

function mockExitedChildProcess(exitCode: number) {
  const stdout = new EventEmitter();
  const stderr = new EventEmitter();

  return {
    stdout,
    stderr,
    on(event: string, handler: (...args: any[]) => void) {
      if (event === 'close') {
        queueMicrotask(() => handler(exitCode));
      }
      return this;
    }
  };
}

const setEnv = (overrides: Record<string, string | undefined>) => {
  for (const [key, value] of Object.entries(overrides)) {
    if (value === undefined) {
      delete process.env[key];
    } else {
      process.env[key] = value;
    }
  }
};

let originalEnv: Record<string, string | undefined>;

const snapshotEnv = () => {
  originalEnv = { ...process.env };
};

const restoreEnv = () => {
  for (const key of Object.keys(process.env)) {
    if (!(key in originalEnv)) {
      delete process.env[key];
    }
  }
  for (const [key, value] of Object.entries(originalEnv)) {
    if (value === undefined) {
      delete process.env[key];
    } else {
      process.env[key] = value;
    }
  }
};

beforeEach(() => {
  snapshotEnv();
  spawnMock.mockReset();
  spawnSyncMock.mockReset();
});

afterEach(() => {
  restoreEnv();
  spawnMock.mockReset();
  spawnSyncMock.mockReset();
  vi.restoreAllMocks();
});

describe('runConfigSet', () => {
  it('deduplicates --clear values into unique entries', async () => {
    spawnMock.mockReturnValueOnce(mockExitedChildProcess(0));

    await runConfigSet({
      clear: ['current_manager', 'current_manager', 'other', 'other', 'other']
    });

    const args = spawnMock.mock.calls[0]![1] as string[];
    expect(args).toEqual([
      'config',
      'set',
      '--clear',
      'current_manager',
      '--clear',
      'other'
    ]);
  });
});

describe('runProfileSet', () => {
  it('maps config override to the CLI --config flag', async () => {
    spawnMock.mockReturnValueOnce(mockExitedChildProcess(0));

    await runProfileSet({
      name: 'api-worker',
      provider: 'github',
      config: '/tmp/gah-config.toml'
    });

    const args = spawnMock.mock.calls[0]![1] as string[];
    expect(args).toContain('--config');
    expect(args).toContain('/tmp/gah-config.toml');
    expect(args).not.toContain('--config-path');
  });
});

describe('loopStateDir', () => {
  it('uses the discovered config parent with .gah-locks location', async () => {
    const tempDir = mkdtempSync(join(tmpdir(), 'gah-config-loopstate-'));
    const configPath = join(tempDir, 'gah-config.toml');
    writeFileSync(configPath, '[profiles]\n');

    setEnv({
      GAH_CONFIG_PATH: configPath,
      GAH_CANONICAL_CONFIG: undefined,
      XDG_STATE_HOME: undefined,
      HOME: undefined
    });

    const expectedFile = join(tempDir, '.gah-locks', 'loop-team-profile.manual-stop.json');
    rmSync(expectedFile, { force: true });

    spawnSyncMock.mockImplementation((command, args) => {
      if (command === 'systemctl' && args.includes('--property=ActiveState')) {
        return {
          status: 0,
          stdout: 'LoadState=loaded\nActiveState=active\nMainPID=42\nActiveEnterTimestamp=manual\n',
          stderr: '',
          error: undefined
        };
      }
      if (command === 'systemctl' && args[1] === 'stop') {
        return { status: 0, stdout: '', stderr: '', error: undefined };
      }
      return { status: 1, stdout: '', stderr: '', error: undefined };
    });

    try {
      const result = await stopLoop('team-profile');

      expect(result).toEqual({ stopped: true });
      expect(existsSync(expectedFile)).toBe(true);
      expect(readFileSync(expectedFile, 'utf8')).toContain('"stoppedAt"');
    } finally {
      rmSync(tempDir, { recursive: true, force: true });
    }
  });
});
