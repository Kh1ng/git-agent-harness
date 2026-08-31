import { useEffect, useRef, useState } from 'react';
import { ChevronUp, Cpu, Star, X } from 'lucide-react';
import type { ManagerBackendInfo, ManagerModelInfo, ManagerReasoningEffortInfo } from '@git-agent-harness/contracts';

/** One saved composer selection. Model/effort are optional snapshots;
 * provider-only favorites keep that provider's configured selection. */
export interface ProviderFavorite {
  backend: string;
  model?: string;
  reasoningEffort?: string;
}

/** The full desired selection after any pick. The parent decides how to
 * apply it (one session PATCH, or a sequence of profile-level calls) —
 * this component owns no server state. */
export interface ProviderSelection {
  backendId: string;
  modelId: string | null;
  reasoningEffortId: string | null;
}

export interface ProviderPickerProps {
  backends: ManagerBackendInfo[];
  selectedBackendId: string | null;
  models: ManagerModelInfo[];
  currentModelId: string | null;
  reasoningEfforts: ManagerReasoningEffortInfo[];
  currentReasoningEffortId: string | null;
  /** False while the selected backend's model list is still loading. */
  modelsLoaded: boolean;
  busy: boolean;
  /** 'session' selections may be null ("default") and a backend switch
   *  resets model + effort to the new backend's defaults; 'profile'
   *  selections always resolve through the profile-level settings. */
  variant: 'session' | 'profile';
  triggerAriaLabel?: string;
  onSelect: (selection: ProviderSelection) => void;
}

const FAVORITES_KEY = 'gah.composer.favorites';

function favoriteKey(favorite: ProviderFavorite): string {
  return `${favorite.backend}\n${favorite.model ?? ''}\n${favorite.reasoningEffort ?? ''}`;
}

function loadFavorites(): ProviderFavorite[] {
  if (typeof window === 'undefined') return [];
  try {
    const raw = window.localStorage.getItem(FAVORITES_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((entry): entry is ProviderFavorite =>
      typeof entry === 'object' && entry !== null
      && typeof (entry as ProviderFavorite).backend === 'string'
    );
  } catch {
    return [];
  }
}

function saveFavorites(favorites: ProviderFavorite[]): void {
  try {
    window.localStorage.setItem(FAVORITES_KEY, JSON.stringify(favorites));
  } catch {
    // Private mode / quota exceeded: favorites just don't persist.
  }
}

/**
 * T3-style provider control for the chat composer: a compact pill reading
 * like "Codex · GPT-5.3 Codex · Medium" that opens a popover with the
 * backend, model, and reasoning-effort choices, plus favorites (persisted
 * in localStorage) that apply all three selections at once.
 *
 * Purely data + callbacks: the same component can drive the session-level
 * pickers here and the factory-config UI later (#1076).
 */
export function ProviderPicker({
  backends,
  selectedBackendId,
  models,
  currentModelId,
  reasoningEfforts,
  currentReasoningEffortId,
  modelsLoaded,
  busy,
  variant,
  triggerAriaLabel = 'Provider picker',
  onSelect
}: ProviderPickerProps) {
  const [open, setOpen] = useState(false);
  const [favorites, setFavorites] = useState<ProviderFavorite[]>(loadFavorites);
  const rootRef = useRef<HTMLDivElement>(null);

  // Click-outside and Escape close the popover while it is open.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open]);

  const selectedModel = models.find((m) => m.id === currentModelId);
  const selectedEffort = reasoningEfforts.find((effort) => effort.id === currentReasoningEffortId);
  const showDefaultModel = variant === 'session' && modelsLoaded && models.length > 0 && !selectedModel;
  const pillLabel = [
    backends.find((b) => b.id === selectedBackendId)?.displayName ?? selectedBackendId ?? 'provider',
    selectedModel?.name ?? (showDefaultModel ? 'Default model' : null),
    selectedEffort?.name ?? (variant === 'session' && reasoningEfforts.length > 0 && !selectedEffort ? 'Default effort' : null)
  ].filter((part): part is string => Boolean(part)).join(' · ');

  const isFavorite = (favorite: ProviderFavorite) =>
    favorites.some((entry) => favoriteKey(entry) === favoriteKey(favorite));

  const toggleFavorite = (favorite: ProviderFavorite) => {
    setFavorites((prev) => {
      const key = favoriteKey(favorite);
      const next = prev.some((entry) => favoriteKey(entry) === key)
        ? prev.filter((entry) => favoriteKey(entry) !== key)
        : [...prev, favorite];
      saveFavorites(next);
      return next;
    });
  };

  const favoriteLabel = (favorite: ProviderFavorite): string => {
    const backend = backends.find((b) => b.id === favorite.backend)?.displayName ?? favorite.backend;
    const model = favorite.model ? models.find((m) => m.id === favorite.model)?.name ?? favorite.model : null;
    const effort = favorite.reasoningEffort
      ? reasoningEfforts.find((e) => e.id === favorite.reasoningEffort)?.name ?? favorite.reasoningEffort
      : null;
    return [backend, model, effort].filter((part): part is string => Boolean(part)).join(' · ');
  };

  const selectBackend = (backendId: string) => {
    // A backend switch resets model + effort: ids are backend-specific, and
    // the new backend answers with its own defaults.
    if (backendId === selectedBackendId) return;
    onSelect({ backendId, modelId: null, reasoningEffortId: null });
  };

  const selectModel = (modelId: string) => {
    if (!selectedBackendId) return;
    onSelect({ backendId: selectedBackendId, modelId, reasoningEffortId: currentReasoningEffortId });
  };

  const selectEffort = (effortId: string) => {
    if (!selectedBackendId) return;
    onSelect({ backendId: selectedBackendId, modelId: currentModelId, reasoningEffortId: effortId });
  };

  const applyFavorite = (favorite: ProviderFavorite) => {
    if (!backends.find((backend) => backend.id === favorite.backend)?.implemented) return;
    onSelect({
      backendId: favorite.backend,
      modelId: favorite.model ?? null,
      reasoningEffortId: favorite.reasoningEffort ?? null
    });
    setOpen(false);
  };

  const currentFavorite: ProviderFavorite | null = selectedBackendId
    && backends.find((backend) => backend.id === selectedBackendId)?.implemented
    ? {
        backend: selectedBackendId,
        ...(currentModelId ? { model: currentModelId } : {}),
        ...(currentReasoningEffortId ? { reasoningEffort: currentReasoningEffortId } : {})
      }
    : null;
  const currentFavoriteSaved = currentFavorite !== null && isFavorite(currentFavorite);

  const starClasses = (saved: boolean) => `shrink-0 ${saved ? 'fill-amber-400 text-amber-400' : 'text-muted'}`;
  const rowClasses = (selected: boolean, enabled: boolean) =>
    `flex min-w-0 flex-1 items-center rounded px-1.5 py-1 text-left text-xs ${
      selected ? 'bg-accent/15 text-primary' : enabled ? 'text-secondary hover:bg-white/5' : 'text-muted'
    } disabled:cursor-not-allowed`;

  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        disabled={busy}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={triggerAriaLabel}
        title={busy ? 'Switching provider is disabled while a turn is in flight' : 'Provider, model, and reasoning effort'}
        className="inline-flex max-w-[14rem] items-center gap-1.5 rounded-md border border-subtle bg-raised px-2 py-2 text-xs text-secondary hover:bg-white/5 disabled:opacity-50"
      >
        <Cpu size={13} className="shrink-0 text-muted" aria-hidden="true" />
        <span className="truncate">{pillLabel}</span>
        <ChevronUp size={12} className="shrink-0 text-muted" aria-hidden="true" />
      </button>
      {open && (
        <div
          role="dialog"
          aria-label={triggerAriaLabel}
          className="absolute bottom-full left-0 z-20 mb-1.5 w-[30rem] max-w-[calc(100vw-3rem)] rounded-md border border-subtle bg-card shadow-lg"
        >
          <div className="border-b border-subtle px-2.5 py-2">
            <div className="flex items-center justify-between gap-2">
              <span className="text-[10px] font-semibold uppercase tracking-wide text-muted">Favorites</span>
              {currentFavorite && (
                <button
                  type="button"
                  onClick={() => toggleFavorite(currentFavorite)}
                  aria-pressed={currentFavoriteSaved}
                  className="inline-flex items-center gap-1 rounded px-1 py-0.5 text-[10px] text-secondary hover:bg-white/5"
                  title={currentFavoriteSaved ? 'Remove the current selection from favorites' : 'Save the current selection as a favorite'}
                >
                  <Star size={11} className={starClasses(currentFavoriteSaved)} aria-hidden="true" />
                  {currentFavoriteSaved ? 'Saved' : 'Save current'}
                </button>
              )}
            </div>
            {favorites.length > 0 ? (
              <div className="mt-1 space-y-0.5">
                {favorites.map((favorite) => {
                  const enabled = backends.find((backend) => backend.id === favorite.backend)?.implemented === true;
                  return <div key={favoriteKey(favorite)} className="flex items-center gap-1">
                    <button
                      type="button"
                      onClick={() => applyFavorite(favorite)}
                      disabled={!enabled}
                      className="min-w-0 flex-1 truncate rounded px-1.5 py-1 text-left text-xs text-secondary hover:bg-white/5 disabled:cursor-not-allowed disabled:opacity-50"
                      aria-label={`Apply ${favoriteLabel(favorite)}`}
                      title={enabled ? 'Apply this favorite' : 'This provider is unavailable'}
                    >
                      <Star size={10} className="mr-1.5 inline fill-amber-400 text-amber-400" aria-hidden="true" />
                      {favoriteLabel(favorite)}
                    </button>
                    <button
                      type="button"
                      onClick={() => toggleFavorite(favorite)}
                      className="shrink-0 rounded p-1 text-muted hover:bg-white/5 hover:text-primary"
                      aria-label={`Remove ${favoriteLabel(favorite)} from favorites`}
                    >
                      <X size={11} aria-hidden="true" />
                    </button>
                  </div>;
                })}
              </div>
            ) : (
              <p className="mt-1 text-[11px] text-muted">No favorites yet — star a provider, model, or the current selection.</p>
            )}
          </div>
          <div className="grid grid-cols-3 gap-2 px-2.5 py-2">
            <div className="min-w-0">
              <p className="text-[10px] font-semibold uppercase tracking-wide text-muted">Provider</p>
              <div className="mt-1 max-h-44 space-y-0.5 overflow-y-auto">
                {backends.map((backend) => {
                  const favorite: ProviderFavorite = { backend: backend.id };
                  const saved = isFavorite(favorite);
                  return (
                    <div key={backend.id} className="flex items-center gap-0.5">
                      <button
                        type="button"
                        onClick={() => selectBackend(backend.id)}
                        disabled={!backend.implemented}
                        aria-current={backend.id === selectedBackendId ? 'true' : undefined}
                        className={rowClasses(backend.id === selectedBackendId, backend.implemented)}
                        title={backend.implemented ? undefined : 'This provider is configured but not wired up yet'}
                      >
                        <span className="truncate">{backend.displayName}{backend.implemented ? '' : ' (unavailable)'}</span>
                      </button>
                      <button
                        type="button"
                        onClick={() => toggleFavorite(favorite)}
                        disabled={!backend.implemented}
                        aria-pressed={saved}
                        aria-label={`Favorite ${backend.displayName}`}
                        className="shrink-0 rounded p-1 text-muted hover:bg-white/5 hover:text-primary disabled:cursor-not-allowed disabled:opacity-50"
                        title="Favorite this provider"
                      >
                        <Star size={11} className={starClasses(saved)} aria-hidden="true" />
                      </button>
                    </div>
                  );
                })}
              </div>
            </div>
            <div className="min-w-0">
              <p className="text-[10px] font-semibold uppercase tracking-wide text-muted">Model</p>
              <div className="mt-1 max-h-44 space-y-0.5 overflow-y-auto">
                {!modelsLoaded && <p className="px-1.5 py-1 text-[11px] text-muted">Loading models…</p>}
                {modelsLoaded && models.length === 0 && (
                  <p className="px-1.5 py-1 text-[11px] text-muted">This provider uses its configured default model.</p>
                )}
                {variant === 'session' && models.length > 0 && (
                  <button
                    type="button"
                    onClick={() => selectedBackendId && onSelect({ backendId: selectedBackendId, modelId: null, reasoningEffortId: currentReasoningEffortId })}
                    aria-current={currentModelId === null ? 'true' : undefined}
                    className={rowClasses(currentModelId === null, true)}
                  >
                    <span className="truncate">Default model</span>
                  </button>
                )}
                {models.map((model) => {
                  const favorite: ProviderFavorite | null = selectedBackendId
                    ? { backend: selectedBackendId, model: model.id }
                    : null;
                  const saved = favorite !== null && isFavorite(favorite);
                  return (
                    <div key={model.id} className="flex items-center gap-0.5">
                      <button
                        type="button"
                        onClick={() => selectModel(model.id)}
                        aria-current={model.id === currentModelId ? 'true' : undefined}
                        className={rowClasses(model.id === currentModelId, true)}
                      >
                        <span className="truncate">{model.name}</span>
                      </button>
                      {favorite && (
                        <button
                          type="button"
                          onClick={() => toggleFavorite(favorite)}
                          aria-pressed={saved}
                          aria-label={`Favorite ${model.name}`}
                          className="shrink-0 rounded p-1 text-muted hover:bg-white/5 hover:text-primary"
                          title="Favorite this provider + model"
                        >
                          <Star size={11} className={starClasses(saved)} aria-hidden="true" />
                        </button>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
            <div className="min-w-0">
              <p className="text-[10px] font-semibold uppercase tracking-wide text-muted">Effort</p>
              <div className="mt-1 max-h-44 space-y-0.5 overflow-y-auto">
                {reasoningEfforts.length === 0 && (
                  <p className="px-1.5 py-1 text-[11px] text-muted">This provider doesn't expose reasoning effort.</p>
                )}
                {variant === 'session' && reasoningEfforts.length > 0 && (
                  <button
                    type="button"
                    onClick={() => selectedBackendId && onSelect({ backendId: selectedBackendId, modelId: currentModelId, reasoningEffortId: null })}
                    aria-current={currentReasoningEffortId === null ? 'true' : undefined}
                    className={rowClasses(currentReasoningEffortId === null, true)}
                  >
                    <span className="truncate">Default effort</span>
                  </button>
                )}
                {reasoningEfforts.map((effort) => {
                  const favorite: ProviderFavorite | null = selectedBackendId
                    ? {
                        backend: selectedBackendId,
                        ...(currentModelId ? { model: currentModelId } : {}),
                        reasoningEffort: effort.id
                      }
                    : null;
                  const saved = favorite !== null && isFavorite(favorite);
                  return (
                    <div key={effort.id} className="flex items-center gap-0.5">
                      <button
                        type="button"
                        onClick={() => selectEffort(effort.id)}
                        aria-current={effort.id === currentReasoningEffortId ? 'true' : undefined}
                        className={rowClasses(effort.id === currentReasoningEffortId, true)}
                        title={effort.description}
                      >
                        <span className="truncate">{effort.name}</span>
                      </button>
                      {favorite && (
                        <button
                          type="button"
                          onClick={() => toggleFavorite(favorite)}
                          aria-pressed={saved}
                          aria-label={`Favorite ${favoriteLabel(favorite)}`}
                          className="shrink-0 rounded p-1 text-muted hover:bg-white/5 hover:text-primary"
                          title="Favorite this provider + model + effort"
                        >
                          <Star size={11} className={starClasses(saved)} aria-hidden="true" />
                        </button>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
