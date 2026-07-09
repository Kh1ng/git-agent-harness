/**
 * Non-emoji icon mapping (lucide-react, already a dependency). Text labels
 * remain the authoritative identifier everywhere these are used -- the
 * icon is a scan aid, not the only signal, matching the "status not
 * communicated by color/icon alone" accessibility rule.
 */
import {
  Github,
  Gitlab,
  Bot,
  Cpu,
  GitBranch,
  Package,
  type LucideIcon
} from 'lucide-react';

export function providerIcon(kind: string): LucideIcon {
  switch (kind) {
    case 'github':
      return Github;
    case 'gitlab':
      return Gitlab;
    case 'codex':
    case 'claude':
    case 'cursor':
    case 'opencode':
    case 'grok':
      return Bot;
    case 'openhands':
    case 'agy':
    case 'vibe':
      return Cpu;
    default:
      return Package;
  }
}

export function modeIcon(mode: string | undefined): LucideIcon {
  switch (mode) {
    case 'improve':
    case 'fix':
      return GitBranch;
    default:
      return Package;
  }
}
