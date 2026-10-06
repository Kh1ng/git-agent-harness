import { useState } from 'react';

export interface AgentModelOption { value: string; label: string }

/** Native keyboard/touch selection, with an explicit escape hatch for unlisted models. */
export function AgentModelSelect({ value, options, label, onChange, loading, failed }: {
  value: string;
  options: AgentModelOption[];
  label: string;
  onChange: (value: string) => void;
  loading?: boolean;
  failed?: boolean;
}) {
  const [custom, setCustom] = useState(false);
  const choices = options.some((option) => option.value === value)
    ? options : [{ value, label: value || 'Choose a model' }, ...options];
  const inputClass = 'w-full min-w-0 rounded-md border border-subtle bg-raised px-3 py-2.5 text-sm text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent';
  return <div className="min-w-0 space-y-2">
    <select aria-label={label} value={custom ? '__custom__' : value} className={inputClass}
      onChange={(event) => {
        if (event.target.value === '__custom__') setCustom(true);
        else { setCustom(false); onChange(event.target.value); }
      }}>
      {choices.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
      <option value="__custom__">Enter another model…</option>
    </select>
    {custom && <input autoFocus aria-label={`Custom ${label.toLowerCase()}`} value={value}
      onChange={(event) => onChange(event.target.value)} placeholder="Exact model name or ID" className={inputClass} />}
    {loading && <p role="status" className="text-xs text-muted">Loading available models…</p>}
    {failed && <p role="status" className="text-xs text-warning">Couldn’t load models. Keep the current model or enter another one.</p>}
  </div>;
}
