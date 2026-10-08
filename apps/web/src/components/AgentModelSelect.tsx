import { useState } from 'react';

export interface AgentModelOption {
  value: string;
  label: string;
  details?: string;
  variants?: { value: string; effort: string; details: string }[];
}

/** Native keyboard/touch selection, with an explicit escape hatch for unlisted models. */
export function AgentModelSelect({ value, options, label, onChange, loading, failed, currentLabel }: {
  value: string;
  options: AgentModelOption[];
  label: string;
  onChange: (value: string) => void;
  loading?: boolean;
  failed?: boolean;
  currentLabel?: string;
}) {
  const [custom, setCustom] = useState(false);
  const selected = options.find((option) => option.value === value || option.variants?.some((variant) => variant.value === value));
  const alias = options.find((option) => option.value.replace(/\[1m\]$/i, '') === value);
  const details = selected?.variants?.find((variant) => variant.value === value)?.details ?? selected?.details
    ?? (alias?.details ? `Configured alias: ${value}\n${alias.details}` : `Configured model: ${value}`);
  const choices = selected
    ? options : [{ value, label: currentLabel || value || 'Choose a model' }, ...options];
  const inputClass = 'w-full min-w-0 rounded-md border border-subtle bg-raised px-3 py-2.5 text-sm text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent';
  return <div className="min-w-0 space-y-2">
    <select aria-label={label} title={details} value={custom ? '__custom__' : selected?.value ?? value} className={inputClass}
      onChange={(event) => {
        if (event.target.value === '__custom__') setCustom(true);
        else {
          setCustom(false);
          const next = options.find((option) => option.value === event.target.value);
          const effort = selected?.variants?.find((variant) => variant.value === value)?.effort;
          onChange(next?.variants?.find((variant) => variant.effort === effort)?.value ?? next?.value ?? event.target.value);
        }
      }}>
      {choices.map((option) => <option key={option.value} value={option.value} title={option.details}>{option.label}</option>)}
      <option value="__custom__">Enter another model…</option>
    </select>
    <details className="text-xs text-muted">
      <summary className="cursor-pointer w-fit" title={details}>Model details</summary>
      <p className="mt-1 whitespace-pre-line break-all rounded-md bg-raised p-2">{details}</p>
    </details>
    {custom && <input autoFocus aria-label={`Custom ${label.toLowerCase()}`} value={value}
      onChange={(event) => onChange(event.target.value)} placeholder="Exact model name or ID" className={inputClass} />}
    {loading && <p role="status" className="text-xs text-muted">Loading available models…</p>}
    {failed && <p role="status" className="text-xs text-warning">Couldn’t load models. Keep the current model or enter another one.</p>}
  </div>;
}
