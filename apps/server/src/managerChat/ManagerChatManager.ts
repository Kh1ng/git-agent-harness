/**
 * Orchestrates one manager-chat conversation per GAH profile: recall memory
 * context, run a Hermes turn (resuming the profile's session if one exists),
 * capture the exchange back to memory. One profile = one ongoing Hermes
 * session, matching "talk to the manager for this repo" from the UI.
 */

import { recall, capture } from './memoryGatewayClient.js';
import { runHermesTurn } from './hermesAdapter.js';

const hermesSessionByProfile = new Map<string, string>();
// Serializes turns per profile -- without this, two concurrent messages for
// the same profile (e.g. two open browser tabs) would both read the same
// resumeSessionId and race to spawn `hermes --resume`, corrupting session
// state. One profile = one conversation, so turns must run one at a time.
const turnQueueByProfile = new Map<string, Promise<unknown>>();

async function runTurn(profile: string, message: string): Promise<string> {
  const { context } = await recall(profile, message);
  const prompt = context ? `Relevant context from prior conversations:\n${context}\n\nUser: ${message}` : message;

  const { sessionId, reply } = await runHermesTurn(prompt, hermesSessionByProfile.get(profile));
  hermesSessionByProfile.set(profile, sessionId);

  await capture(profile, message, reply);
  return reply;
}

export function sendManagerChatMessage(profile: string, message: string): Promise<string> {
  const prior = turnQueueByProfile.get(profile) ?? Promise.resolve();
  const turn = prior.catch(() => undefined).then(() => runTurn(profile, message));
  turnQueueByProfile.set(profile, turn.catch(() => undefined));
  return turn;
}
