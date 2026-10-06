import type { ManagerModelInfo } from '@git-agent-harness/contracts';
import type { AgentModelOption } from '../components/AgentModelSelect.js';
import { agentModelLabel } from './agentModelLabel.js';

export function taskReasoningEfforts(backend: string, advertised: { id: string; name: string }[] = []): { id: string; name: string }[] {
  // Claude's ACP chat catalog can omit effort even though task dispatch uses
  // the native --effort flag. CLI levels: code.claude.com/docs/en/model-config.
  return backend === 'claude' && advertised.length === 0
    ? ['low', 'medium', 'high', 'xhigh', 'max'].map((id) => ({ id, name: id }))
    : advertised;
}

export function agentModelOptions(backend: string, models: ManagerModelInfo[], aliases: { backend: string; alias: string; model: string }[]): AgentModelOption[] {
  const options: AgentModelOption[] = [];
  for (const model of models.filter((model) => model.id !== 'default')) {
    const resolved = aliases.find((alias) => alias.backend === backend
      && (alias.alias === model.id || alias.alias === model.id.replace(/\[1m\]$/i, ''))
      && /^claude-(?:opus|sonnet|haiku|fable)-\d/.test(alias.model))?.model;
    const details = `Provider name: ${model.name}\nModel ID: ${model.id}${resolved ? `\nLast recorded model: ${resolved}` : ''}${model.description ? `\n${model.description}` : ''}`;
    if (/^agy(?:[:_-]|$)/i.test(backend)) {
      // Only collapse the explicit effort suffix the provider advertises.
      const variant = /^(.*?) \((Low|Medium|High|Xhigh|Max|Ultra)\)$/i.exec(model.name);
      if (variant) {
        const label = variant[1].replace(/^gpt(?=[ -])/i, 'GPT');
        let group = options.find((option) => option.label === label && option.variants);
        if (!group) {
          group = { value: model.name, label, details, variants: [] };
          options.push(group);
        }
        group.variants!.push({ value: model.name, effort: variant[2].toLowerCase(), details });
        // Medium is the initial choice where available; configured variants stay intact.
        if (variant[2].toLowerCase() === 'medium') group.value = model.name;
        continue;
      }
    }
    options.push({ value: /^agy(?:[:_-]|$)/i.test(backend) ? model.name : model.id, label: agentModelLabel(backend, model, resolved), details });
  }
  const effortOrder = ['low', 'medium', 'high', 'xhigh', 'max', 'ultra'];
  for (const option of options) option.variants?.sort((a, b) => effortOrder.indexOf(a.effort) - effortOrder.indexOf(b.effort));
  return options;
}
