import type { ManagerModelInfo } from '@git-agent-harness/contracts';

/** Use the provider's version metadata without turning an alias into a pinned model ID. */
export function agentModelLabel(backend: string, model: ManagerModelInfo, resolvedModel?: string): string {
  if (!/^claude(?:[:_-]|$)/i.test(backend)) {
    return model.name === model.id ? model.name : `${model.name} (${model.id})`;
  }
  const family = /^(?:claude[- ]?)?(opus|sonnet|haiku|fable)(?=$|[- [\d])/i.exec(model.id)
    ?? /^(?:Claude\s+)?(Opus|Sonnet|Haiku|Fable)\b/i.exec(model.name);
  if (!family) return model.name;
  const name = family[1][0].toUpperCase() + family[1].slice(1).toLowerCase();
  const idVersion = new RegExp(`^(?:claude-)?${family[1]}-(\\d{1,2}(?:[.-]\\d{1,2})?)(?:$|[-\\[])`, 'i').exec(model.id)?.[1];
  const textVersion = new RegExp(`^(?:Claude\\s+)?${family[1]}\\s+(\\d+(?:\\.\\d+)?)(?=$|\\s|\\()`, 'i');
  let version = idVersion?.replace('-', '.') ?? textVersion.exec(model.name)?.[1]
    ?? textVersion.exec(model.description ?? '')?.[1];
  const resolvedVersion = new RegExp(`^claude-${family[1]}-(\\d{1,2})[.-](\\d{1,2})(?:$|[-\\[])`, 'i').exec(resolvedModel ?? '');
  // A resolved alias can refine a broad major version; never replace a pinned ID
  // or a newer/specific version supplied by the provider with historical usage.
  if (!idVersion && resolvedVersion && (!version || version === resolvedVersion[1])) {
    version = `${resolvedVersion[1]}.${resolvedVersion[2]}`;
  }
  const context = /\[1m\]/i.test(model.id) || /1M context/i.test(model.name) ? ' (1M context)' : '';
  const variant = /thinking/i.test(model.name) ? ' (Thinking)' : '';
  return version ? `${name} ${version}${context}${variant}` : `${name}${context}${variant} (provider default)`;
}

export function currentAgentModelLabel(backend: string, value: string, options: { value: string; label: string }[]): string {
  const alias = /^claude(?:[:_-]|$)/i.test(backend)
    ? options.find((option) => option.value.replace(/\[1m\]$/i, '') === value) : undefined;
  if (alias) return `${alias.label.replace(/ \(1M context\)/g, '')} (provider default)`;
  return agentModelLabel(backend, { id: value, name: value });
}
