/**
 * Orchestrates one manager-chat conversation per GAH profile: recall memory
 * context, run a Hermes turn (resuming the profile's session if one exists),
 * capture the exchange back to memory. One profile = one ongoing Hermes
 * session, matching "talk to the manager for this repo" from the UI.
 */

import { recall, capture } from './memoryGatewayClient.js';
import { runHermesTurn } from './hermesAdapter.js';

const hermesSessionByProfile = new Map<string, string>();

export async function sendManagerChatMessage(profile: string, message: string): Promise<string> {
  const { context } = await recall(profile, message);
  const prompt = context ? `Relevant context from prior conversations:\n${context}\n\nUser: ${message}` : message;

  const { sessionId, reply } = await runHermesTurn(prompt, hermesSessionByProfile.get(profile));
  hermesSessionByProfile.set(profile, sessionId);

  await capture(profile, message, reply);
  return reply;
}
