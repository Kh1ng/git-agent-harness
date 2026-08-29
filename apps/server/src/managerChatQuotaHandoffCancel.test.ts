/**
 * Issue #1025: a quota handoff to a fallback backend that never settles must
 * still be cancellable within the existing cancel-settle deadline, instead of
 * throwing "cancel timed out waiting for backend to stop" and wedging the
 * conversation.
 *
 * Deterministic reproduction: monkey-patch the real `hermes`/`codex`
 * adapters (the registry's own singletons -- resolveAdapter always returns
 * the same object) so `hermes.runTurn` rejects with a usage-limit error and
 * `codex.runTurn`/`codex.cancelTurn` never resolve, exactly like a wedged
 * codex-acp process. No process spawning needed for this path: the handoff
 * and cancel wiring under test lives entirely in ManagerChatManager.ts.
 */
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { resolveAdapter } from './managerChat/registry.js';
import {
  sendManagerChatMessage,
  cancelManagerChatTurn,
  setSessionLogOptions
} from './managerChat/ManagerChatManager.js';
import { loadLog } from './managerChat/sessionLog.js';

const fixtures = resolve(dirname(fileURLToPath(import.meta.url)), '../tests/fixtures');

test('cancelling a quota-handoff turn stuck on a never-settling fallback settles as cancelled, not a timeout error', { timeout: 20_000 }, async () => {
  const stateDir = mkdtempSync(join(tmpdir(), 'gah-chat-handoff-cancel-'));
  const savedEnv = { ...process.env };
  // Isolate every shell-out and network call from this real dev machine's
  // state: a fixture `gah` binary (never the real CLI/profiles) and an
  // unreachable memory gateway (fails open, fast).
  process.env.GAH_BINARY = join(fixtures, 'gah', 'gah');
  process.env.GAH_GATEWAY_SETTINGS_PATH = join(stateDir, 'gateway.json');
  process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = join(stateDir, 'manager-chat.json');
  process.env.TDAI_GATEWAY_URL = 'http://127.0.0.1:1';
  setSessionLogOptions({ stateDir: join(stateDir, 'chat') });

  const hermes = resolveAdapter('hermes');
  const codex = resolveAdapter('codex');
  const originalHermesRunTurn = hermes.runTurn;
  const originalHermesCancelTurn = hermes.cancelTurn;
  const originalCodexRunTurn = codex.runTurn;
  const originalCodexCancelTurn = codex.cancelTurn;

  let codexEntered = false;
  let codexCancelInvoked = false;
  let hermesCancelInvoked = false;
  hermes.runTurn = async () => {
    throw new Error("You've hit your usage limit. Please wait or upgrade.");
  };
  hermes.cancelTurn = async () => { hermesCancelInvoked = true; };
  // The pathological fallback (#1025): neither the turn nor its cancel ever
  // resolves, mirroring a wedged codex-acp child process.
  codex.runTurn = () => { codexEntered = true; return new Promise<never>(() => {}); };
  codex.cancelTurn = () => { codexCancelInvoked = true; return new Promise<void>(() => {}); };

  const profile = `handoff-cancel-${Date.now()}`;
  try {
    const turnPromise = sendManagerChatMessage(profile, 'do the thing', 'req-1');

    // Wait for the handoff to actually reach the fallback (deterministic
    // polling, not a fixed sleep) before cancelling it.
    const deadline = Date.now() + 5_000;
    while (!codexEntered && Date.now() < deadline) {
      await new Promise((r) => setTimeout(r, 5));
    }
    assert.ok(codexEntered, 'handoff reached the fallback (codex) backend');

    // Both the cancel round trip and the turn's own settle race the same
    // CANCEL_SETTLE_TIMEOUT_MS deadline against the wedged adapter; the
    // deadline's timers are deliberately unref()'d in production (a wedged
    // backend must never keep the real server process alive by itself) --
    // this bare test has nothing else pumping the event loop, so keep it
    // alive ourselves for the duration of both waits.
    const keepAlive = setInterval(() => {}, 100);
    let cancelled: boolean;
    let result: Awaited<typeof turnPromise>;
    try {
      cancelled = await cancelManagerChatTurn(profile);
      assert.equal(cancelled, true, 'a turn was in flight to cancel');
      assert.ok(codexCancelInvoked, 'cancel targets the active fallback adapter, not the original backend');
      assert.equal(hermesCancelInvoked, false, 'the exhausted start backend is never asked to cancel');

      // The turn must SETTLE (resolve as cancelled) within the existing
      // cancel-settle deadline, never reject with "cancel timed out...".
      result = await turnPromise;
    } finally {
      clearInterval(keepAlive);
    }
    assert.equal(result.cancelled, true);
    assert.equal(result.turn.text, '');

    const events = loadLog(profile, { stateDir: join(stateDir, 'chat') });
    const ends = events.filter((e) => e.type === 'turn/end');
    assert.equal(ends.length, 1, 'exactly one terminal event for the turn');
    assert.equal(ends[0].type === 'turn/end' && ends[0].reason.kind, 'cancelled');
    // The gateway is deliberately unreachable in this test (isolation from
    // real machine state), so a "recall degraded" harness/error is expected
    // noise -- what must never appear is the cancel-timeout error this test
    // guards against.
    assert.ok(
      events.every((e) => e.type !== 'harness/error' || !e.text.includes('cancel timed out')),
      'no "cancel timed out" error event for a cancelled handoff'
    );

    // The next turn on the same conversation must run normally.
    hermes.runTurn = async () => ({ reply: 'ok', model: null, usage: null });
    const next = await sendManagerChatMessage(profile, 'hello again', 'req-2');
    assert.equal(next.cancelled, false);
    assert.equal(next.turn.text, 'ok');
  } finally {
    hermes.runTurn = originalHermesRunTurn;
    hermes.cancelTurn = originalHermesCancelTurn;
    codex.runTurn = originalCodexRunTurn;
    codex.cancelTurn = originalCodexCancelTurn;
    setSessionLogOptions({ stateDir: undefined });
    process.env = savedEnv;
    rmSync(stateDir, { recursive: true, force: true });
  }
});
