import { useEffect, useRef, useState } from 'react';
import { ChevronUp, Cpu, LayoutGrid, Star, X } from 'lucide-react';
import type { ManagerBackendInfo, ManagerModelInfo, ManagerReasoningEffortInfo } from '@git-agent-harness/contracts';
import { ModelBrowser, type ModelCatalogSource } from './ModelBrowser.js';

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
  selectedInstanceLabel?: string | null;
  models: ManagerModelInfo[];
  currentModelId: string | null;
  reasoningEfforts: ManagerReasoningEffortInfo[];
  currentReasoningEffortId: string | null;
  /** False while the selected backend's model list is still loading. */
  modelsLoaded: boolean;
  busy: boolean;
  /** 'backend' shows only the shared backend list. 'session' selections may be null ("default") and a backend switch
   *  resets model + effort to the new backend's defaults; 'profile'
   *  selections always resolve through the profile-level settings. */
  variant: 'backend' | 'session' | 'profile';
  triggerAriaLabel?: string;
  /** Provider context the full catalog is read through. Omitted (or on
   *  the 'backend' variant) "Browse all models" is not offered. */
  catalog?: ModelCatalogSource;
  onSelect: (selection: ProviderSelection) => void;
}

const FAVORITES_KEY = 'gah.composer.favorites';
const RECENTS_KEY = 'gah.composer.recents';
const QUICK_CHOICES_SHOWN = 6;
const RECENTS_KEPT = 6;

function favoriteKey(favorite: ProviderFavorite): string {
  return `${favorite.backend}\n${favorite.model ?? ''}\n${favorite.reasoningEffort ?? ''}`;
}

function loadStored(key: string): ProviderFavorite[] {
  if (typeof window === 'undefined') return [];
  try {
    const raw = window.localStorage.getItem(key);
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

function save(key: string, entries: ProviderFavorite[]): void {
  try {
    window.localStorage.setItem(key, JSON.stringify(entries));
  } catch {
    // Private mode / quota exceeded: the list just doesn't persist.
  }
}

/**
 * Shared provider control: a compact pill reading
 * like "Codex · GPT-5.3 Codex · Medium" that opens a short popover — recent
 * picks, starred favorites, and reasoning effort. The full provider/model
 * catalog lives one step further, behind "Browse all models" (#1203), so
 * the common switch never renders every model the fleet can reach.
 *
 * Purely data + callbacks: the same component can drive the session-level
 * pickers here and the factory-config UI later (#1076).
 */
export function ProviderPicker({
  backends,
  selectedBackendId,
  selectedInstanceLabel,
  models,
  currentModelId,
  reasoningEfforts,
  currentReasoningEffortId,
  modelsLoaded,
  busy,
  variant,
  triggerAriaLabel = 'Provider picker',
  catalog,
  onSelect
}: ProviderPickerProps) {
  const [open, setOpen] = useState(false);
  /** The pill sits at the bottom of the composer here and at the top of a
   * form elsewhere, so the popover flips to whichever side has room
   * instead of rendering off-screen. */
  const [dropUp, setDropUp] = useState(true);
  const [browsing, setBrowsing] = useState(false);
  const [favorites, setFavorites] = useState<ProviderFavorite[]>(() => loadStored(FAVORITES_KEY));
  const [recents, setRecents] = useState<ProviderFavorite[]>(() => loadStored(RECENTS_KEY));
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  // Click-outside and Escape close the popover while it is open. The
  // browser dialog owns its own dismissal, so it is excluded here.
  useEffect(() => {
    if (!open || browsing) return;
    const onPointerDown = (event: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpen(false);
        queueMicrotask(() => triggerRef.current?.focus());
      }
    };
    document.addEventListener('mousedown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open, browsing]);

  const selectedModel = models.find((m) => m.id === currentModelId);
  const selectedEffort = reasoningEfforts.find((effort) => effort.id === currentReasoningEffortId);
  const showDefaultModel = variant === 'session' && modelsLoaded && models.length > 0 && !selectedModel;
  const backendName = (id: string) => backends.find((backend) => backend.id === id)?.displayName ?? id;
  const pillLabel = [
    selectedBackendId ? backendName(selectedBackendId) : 'provider',
    selectedInstanceLabel,
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
      save(FAVORITES_KEY, next);
      return next;
    });
  };

  const rememberRecent = (entry: ProviderFavorite) => {
    setRecents((prev) => {
      const key = favoriteKey(entry);
      const next = [entry, ...prev.filter((item) => favoriteKey(item) !== key)].slice(0, RECENTS_KEPT);
      save(RECENTS_KEY, next);
      return next;
    });
  };

  const entryLabel = (entry: ProviderFavorite): string => {
    const model = entry.model ? models.find((m) => m.id === entry.model)?.name ?? entry.model : null;
    const effort = entry.reasoningEffort
      ? reasoningEfforts.find((e) => e.id === entry.reasoningEffort)?.name ?? entry.reasoningEffort
      : null;
    return [backendName(entry.backend), model, effort].filter((part): part is string => Boolean(part)).join(' · ');
  };

  const selectEffort = (effortId: string) => {
    if (!selectedBackendId) return;
    onSelect({ backendId: selectedBackendId, modelId: currentModelId, reasoningEffortId: effortId });
    rememberRecent({
      backend: selectedBackendId,
      ...(currentModelId ? { model: currentModelId } : {}),
      reasoningEffort: effortId
    });
  };

  const selectBackend = (backendId: string) => {
    // A backend switch resets model + effort: ids are backend-specific, and
    // the new backend answers with its own defaults.
    if (backendId === selectedBackendId) return;
    onSelect({ backendId, modelId: null, reasoningEffortId: null });
  };

  /** A recent or favorite entry carries the whole selection; anything it
   * omits falls back to the target backend's own default. */
  const applyEntry = (entry: ProviderFavorite) => {
    if (!backends.find((backend) => backend.id === entry.backend)?.implemented) return;
    onSelect({
      backendId: entry.backend,
      modelId: entry.model ?? null,
      reasoningEffortId: entry.reasoningEffort ?? null
    });
    rememberRecent(entry);
    setOpen(false);
    queueMicrotask(() => triggerRef.current?.focus());
  };

  /** A catalog pick keeps the effort when the provider is unchanged; a
   * different provider resolves its own default effort. */
  const applyBrowsed = (backendId: string, modelId: string | null) => {
    const effort = backendId === selectedBackendId ? currentReasoningEffortId : null;
    onSelect({ backendId, modelId, reasoningEffortId: effort });
    rememberRecent({
      backend: backendId,
      ...(modelId ? { model: modelId } : {}),
      ...(effort ? { reasoningEffort: effort } : {})
    });
    setOpen(false);
  };

  const currentEntry: ProviderFavorite | null = selectedBackendId
    && backends.find((backend) => backend.id === selectedBackendId)?.implemented
    ? {
        backend: selectedBackendId,
        ...(currentModelId ? { model: currentModelId } : {}),
        ...(currentReasoningEffortId ? { reasoningEffort: currentReasoningEffortId } : {})
      }
    : null;
  const currentSaved = currentEntry !== null && isFavorite(currentEntry);

  const quickFavorites = favorites.slice(0, QUICK_CHOICES_SHOWN);
  const quickRecents = recents
    .filter((entry) => !isFavorite(entry) && backends.some((backend) => backend.id === entry.backend && backend.implemented))
    .filter((entry) => currentEntry === null || favoriteKey(entry) !== favoriteKey(currentEntry))
    .slice(0, QUICK_CHOICES_SHOWN - quickFavorites.length);

  const togglePopover = () => {
    const bounds = triggerRef.current?.getBoundingClientRect();
    if (bounds) setDropUp(bounds.top > window.innerHeight - bounds.bottom);
    setOpen((v) => !v);
  };

  const starClasses = (saved: boolean) => `shrink-0 ${saved ? 'fill-amber-400 text-amber-400' : 'text-muted'}`;
  const rowClasses = (selected: boolean, enabled: boolean) =>
    `touch-target flex min-w-0 flex-1 items-center rounded px-1.5 py-1 text-left text-xs max-sm:min-h-11 max-sm:min-w-11 ${
      selected ? 'bg-accent/15 text-primary' : enabled ? 'text-secondary hover:bg-white/5' : 'text-muted'
    } disabled:cursor-not-allowed`;
  const sectionLabel = 'text-[10px] font-semibold uppercase tracking-wide text-muted';

  return (
    <div ref={rootRef} className="relative">
      <button
        ref={triggerRef}
        type="button"
        onClick={togglePopover}
        disabled={busy}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={triggerAriaLabel}
        title={busy ? 'Switching provider is disabled while a turn is in flight' : variant === 'backend' ? 'Provider' : 'Provider, model, and reasoning effort'}
        className={`touch-target inline-flex max-w-[14rem] ${variant === 'backend' ? '' : 'max-sm:max-w-[5.5rem]'} max-sm:min-h-11 max-sm:min-w-11 items-center gap-1.5 rounded-md border border-subtle bg-raised px-2 py-2 text-xs text-secondary hover:bg-white/5 disabled:opacity-50`}
      >
        <Cpu size={13} className="shrink-0 text-muted" aria-hidden="true" />
        <span className="truncate">{pillLabel}</span>
        <ChevronUp size={12} className="shrink-0 text-muted" aria-hidden="true" />
      </button>
      {open && (
        <div
          role="dialog"
          aria-label={triggerAriaLabel}
          className={`fixed inset-x-3 bottom-3 z-20 max-h-[calc(100dvh-1.5rem)] overflow-y-auto rounded-md border border-subtle bg-card shadow-lg sm:absolute sm:left-0 sm:right-auto sm:max-h-[70vh] sm:w-[22rem] sm:max-w-[calc(100vw-3rem)] ${
            dropUp ? 'sm:bottom-full sm:mb-1.5' : 'sm:bottom-auto sm:top-full sm:mt-1.5'
          }`}
        >
          <div className="flex items-center justify-between border-b border-subtle px-2.5 sm:hidden">
            <span className="text-xs font-semibold text-primary">
              {variant === 'backend' ? 'Choose provider' : 'Choose provider, model, and effort'}
            </span>
            <button
              type="button"
              onClick={() => { setOpen(false); triggerRef.current?.focus(); }}
              className="touch-target inline-flex items-center justify-center rounded text-muted hover:bg-white/5 hover:text-primary max-sm:min-h-11 max-sm:min-w-11"
              aria-label="Close provider picker"
            >
              <X size={16} aria-hidden="true" />
            </button>
          </div>
          <fieldset disabled={busy} className="contents">
          {variant === 'backend' ? (
            <div className="px-2.5 py-2">
              <p className={sectionLabel}>Provider</p>
              <div className="mt-1 max-h-60 space-y-0.5 overflow-y-auto">
                {backends.map((backend) => (
                  <button
                    key={backend.id}
                    type="button"
                    onClick={() => selectBackend(backend.id)}
                    disabled={!backend.implemented}
                    aria-current={backend.id === selectedBackendId ? 'true' : undefined}
                    className={rowClasses(backend.id === selectedBackendId, backend.implemented)}
                    title={backend.implemented ? undefined : 'This provider is configured but not wired up yet'}
                  >
                    <span className="truncate">{backend.displayName}{backend.implemented ? '' : ' (unavailable)'}</span>
                  </button>
                ))}
              </div>
            </div>
          ) : (
            <>
              <div className="border-b border-subtle px-2.5 py-2">
                <div className="flex items-center justify-between gap-2">
                  <span className={sectionLabel}>Current</span>
                  {currentEntry && (
                    <button
                      type="button"
                      onClick={() => toggleFavorite(currentEntry)}
                      aria-pressed={currentSaved}
                      className="touch-target inline-flex items-center gap-1 rounded px-1 py-0.5 text-[10px] text-secondary hover:bg-white/5 max-sm:min-h-11 max-sm:min-w-11"
                      title={currentSaved ? 'Remove the current selection from favorites' : 'Save the current selection as a favorite'}
                    >
                      <Star size={11} className={starClasses(currentSaved)} aria-hidden="true" />
                      {currentSaved ? 'Saved' : 'Save current'}
                    </button>
                  )}
                </div>
                <p className="mt-0.5 truncate text-sm text-primary">{pillLabel}</p>
              </div>

              {(quickFavorites.length > 0 || quickRecents.length > 0) && (
                <div className="border-b border-subtle px-2.5 py-2">
                  {quickFavorites.length > 0 && <>
                    <p className={sectionLabel}>Favorites</p>
                    <div className="mt-1 space-y-0.5">
                      {quickFavorites.map((favorite) => {
                        const enabled = backends.find((backend) => backend.id === favorite.backend)?.implemented === true;
                        return <div key={favoriteKey(favorite)} className="flex items-center gap-1">
                          <button
                            type="button"
                            onClick={() => applyEntry(favorite)}
                            disabled={!enabled}
                            className="touch-target min-w-0 flex-1 truncate rounded px-1.5 py-1 text-left text-xs text-secondary hover:bg-white/5 disabled:cursor-not-allowed disabled:opacity-50 max-sm:min-h-11 max-sm:min-w-11"
                            aria-label={`Apply ${entryLabel(favorite)}`}
                            title={enabled ? 'Apply this favorite' : 'This provider is unavailable'}
                          >
                            <Star size={10} className="mr-1.5 inline fill-amber-400 text-amber-400" aria-hidden="true" />
                            {entryLabel(favorite)}
                          </button>
                          <button
                            type="button"
                            onClick={() => toggleFavorite(favorite)}
                            className="touch-target shrink-0 rounded p-1 text-muted hover:bg-white/5 hover:text-primary max-sm:min-h-11 max-sm:min-w-11"
                            aria-label={`Remove ${entryLabel(favorite)} from favorites`}
                          >
                            <X size={11} aria-hidden="true" />
                          </button>
                        </div>;
                      })}
                    </div>
                  </>}
                  {quickRecents.length > 0 && (
                    <div className={quickFavorites.length > 0 ? 'mt-2.5' : ''}>
                      <p className={sectionLabel}>Recent</p>
                      <div className="mt-1 space-y-0.5">
                        {quickRecents.map((entry) => (
                          <div key={favoriteKey(entry)} className="flex items-center gap-1">
                            <button
                              type="button"
                              onClick={() => applyEntry(entry)}
                              className="touch-target min-w-0 flex-1 truncate rounded px-1.5 py-1 text-left text-xs text-secondary hover:bg-white/5 max-sm:min-h-11 max-sm:min-w-11"
                              aria-label={`Apply ${entryLabel(entry)}`}
                            >
                              {entryLabel(entry)}
                            </button>
                            <button
                              type="button"
                              onClick={() => toggleFavorite(entry)}
                              aria-pressed={isFavorite(entry)}
                              aria-label={`Favorite ${entryLabel(entry)}`}
                              className="touch-target shrink-0 rounded p-1 text-muted hover:bg-white/5 hover:text-primary max-sm:min-h-11 max-sm:min-w-11"
                            >
                              <Star size={11} className={starClasses(isFavorite(entry))} aria-hidden="true" />
                            </button>
                          </div>
                        ))}
                      </div>
                    </div>
                  )}
                </div>
              )}

              {reasoningEfforts.length > 0 && (
                <div className="border-b border-subtle px-2.5 py-2">
                  <p className={sectionLabel}>Effort</p>
                  <div className="mt-1 flex flex-wrap gap-1">
                    {variant === 'session' && (
                      <button
                        type="button"
                        onClick={() => selectedBackendId && onSelect({ backendId: selectedBackendId, modelId: currentModelId, reasoningEffortId: null })}
                        aria-current={currentReasoningEffortId === null ? 'true' : undefined}
                        className={`touch-target rounded-full border px-2.5 py-1 text-xs max-sm:min-h-11 max-sm:min-w-11 max-sm:px-4 ${
                          currentReasoningEffortId === null ? 'border-accent/50 bg-accent/15 text-primary' : 'border-subtle text-secondary hover:bg-white/5'
                        }`}
                      >
                        Default
                      </button>
                    )}
                    {reasoningEfforts.map((effort) => (
                      <button
                        key={effort.id}
                        type="button"
                        onClick={() => selectEffort(effort.id)}
                        aria-current={effort.id === currentReasoningEffortId ? 'true' : undefined}
                        title={effort.description}
                        className={`touch-target rounded-full border px-2.5 py-1 text-xs max-sm:min-h-11 max-sm:min-w-11 max-sm:px-4 ${
                          effort.id === currentReasoningEffortId ? 'border-accent/50 bg-accent/15 text-primary' : 'border-subtle text-secondary hover:bg-white/5'
                        }`}
                      >
                        {effort.name}
                      </button>
                    ))}
                  </div>
                </div>
              )}

              {catalog && (
                <div className="px-2.5 py-2">
                  <button
                    type="button"
                    onClick={() => { setOpen(false); setBrowsing(true); }}
                    className="touch-target flex w-full items-center justify-center gap-1.5 rounded-md border border-subtle px-2 py-2 text-xs text-secondary hover:border-accent/40 hover:text-primary max-sm:min-h-11 max-sm:min-w-11"
                  >
                    <LayoutGrid size={13} aria-hidden="true" /> Browse all models
                  </button>
                </div>
              )}
            </>
          )}
          </fieldset>
        </div>
      )}
      {catalog && variant !== 'backend' && (
        <ModelBrowser
          open={browsing}
          backends={backends}
          source={catalog}
          selectedBackendId={selectedBackendId}
          selectedModelId={currentModelId}
          isFavorite={isFavorite}
          onToggleFavorite={toggleFavorite}
          onPick={applyBrowsed}
          onClose={() => { setBrowsing(false); triggerRef.current?.focus(); }}
        />
      )}
    </div>
  );
}
