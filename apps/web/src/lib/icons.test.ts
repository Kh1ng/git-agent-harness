import { Bot, Cpu, Github, Gitlab, GitBranch, Package, type LucideIcon } from 'lucide-react';
import { modeIcon, providerIcon } from './icons.js';

const providerCases: [string, LucideIcon][] = [
  ['github', Github],
  ['gitlab', Gitlab],
  ...['codex', 'claude', 'cursor', 'opencode', 'grok'].map((kind): [string, LucideIcon] => [kind, Bot]),
  ...['openhands', 'agy', 'vibe'].map((kind): [string, LucideIcon] => [kind, Cpu])
];

test.each(providerCases)('maps provider %s', (kind, icon) => {
  expect(providerIcon(kind)).toBe(icon);
});
test.each(['', 'unknown', '猫'])('uses the fallback for provider %s', (kind) => {
  expect(providerIcon(kind)).toBe(Package);
});
test.each(['fix', 'improve'])('maps mode %s', (mode) => expect(modeIcon(mode)).toBe(GitBranch));
test.each([undefined, '', 'review'])('uses the fallback for mode %s', (mode) => expect(modeIcon(mode)).toBe(Package));
