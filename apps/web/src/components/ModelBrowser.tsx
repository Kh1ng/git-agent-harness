import { useEffect, useMemo, useRef, useState } from 'react';
import { Search, Star, X } from 'lucide-react';
import type { ManagerBackendInfo, ManagerModelInfo } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';
import type { ProviderFavorite } from './ProviderPicker.js';

/** One selectable row of the full catalog. `model: null` is the backend's
 * own configured default — the only way to reach a provider that exposes
 * no model picker at all. */
export interface ModelCatalogEntry {
  backend: ManagerBackendInfo;
  model: ManagerModelInfo | null;
}

/** Which conversation's provider context the catalog is read through.
 * Model lists are per project and per node, so the browser needs both. */
export interface ModelCatalogSource {
  profile: string;
  nodeId?: string;
}

export interface ModelBrowserProps {
  open: boolean;
  backends: ManagerBackendInfo[];
  source: ModelCatalogSource;
  selectedBackendId: string | null;
  selectedModelId: string | null;
  isFavorite: (favorite: ProviderFavorite) => boolean;
  onToggleFavorite: (favorite: ProviderFavorite) => void;
  onPick: (backendId: string, modelId: string | null) => void;
  onClose: () => void;
}

export function entryFavorite(entry: ModelCatalogEntry): ProviderFavorite {
  return { backend: entry.backend.id, ...(entry.model ? { model: entry.model.id } : {}) };
}

const DEFAULT_MODEL_NAME = 'Default model';

/**
 * The full provider/model catalog (#1203): every implemented backend's
 * models in one searchable grid, so the composer pill can stay down to the
 * selection plus a few favorites.
 *
 * Instance identity lives in the backend's own display name, which every
 * card carries — two instances exposing the same model stay distinguishable.
 */
export function ModelBrowser({
  open,
  backends,
  source,
  selectedBackendId,
  selectedModelId,
  isFavorite,
  onToggleFavorite,
  onPick,
  onClose
}: ModelBrowserProps) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [catalog, setCatalog] = useState<Record<string, ManagerModelInfo[]>>({});
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState<string[]>([]);
  const [retryEpoch, setRetryEpoch] = useState(0);
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState('all');

  useEffect(() => {
    if (open) dialog.current?.showModal();
    else dialog.current?.close();
  }, [open]);

  useEffect(() => {
    if (!open) return;
    setQuery('');
    setFilter('all');
  }, [open]);

  const implemented = useMemo(() => backends.filter((backend) => backend.implemented), [backends]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    // One read per provider instance: the same per-backend endpoint the
    // composer already uses, asked for every instance instead of only the
    // active one. A rejection degrades that provider to its default entry.
    Promise.all(implemented.map((backend) =>
      gahApi.getManagerChatModelsForBackend(source.profile, backend.id, source.nodeId)
        .then(({ models }) => [backend.id, models] as const)
        .catch(() => [backend.id, null] as const)
    )).then((results) => {
      if (cancelled) return;
      setCatalog(Object.fromEntries(results.map(([id, models]) => [id, models ?? []])));
      setFailed(results.filter(([, models]) => models === null).map(([id]) => id));
      setLoading(false);
    });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, retryEpoch, implemented, source.profile, source.nodeId]);

  // A configured-but-unwired provider still appears, flagged and
  // unselectable (#945) -- silently dropping it reads as "not configured".
  const entries = useMemo<ModelCatalogEntry[]>(() => backends.flatMap((backend) => backend.implemented
    ? [{ backend, model: null }, ...(catalog[backend.id] ?? []).map((model) => ({ backend, model }))]
    : [{ backend, model: null }]), [backends, catalog]);

  const needle = query.trim().toLowerCase();
  const visible = entries.filter((entry) => {
    if (filter === 'favorites' && !isFavorite(entryFavorite(entry))) return false;
    if (filter !== 'all' && filter !== 'favorites' && entry.backend.id !== filter) return false;
    if (!needle) return true;
    const haystack = [
      entry.backend.displayName,
      entry.backend.id,
      entry.model?.name ?? DEFAULT_MODEL_NAME,
      entry.model?.id ?? '',
      entry.model?.description ?? ''
    ].join(' ').toLowerCase();
    return haystack.includes(needle);
  });

  const chip = (id: string, label: string) => (
    <button
      key={id}
      type="button"
      onClick={() => setFilter(id)}
      aria-pressed={filter === id}
      className={`touch-target shrink-0 rounded-full border px-3 py-1 text-xs transition-colors max-sm:min-h-11 max-sm:min-w-11 max-sm:px-4 ${
        filter === id
          ? 'border-accent/50 bg-accent/15 text-primary'
          : 'border-subtle text-secondary hover:border-accent/30 hover:text-primary'
      }`}
    >
      {label}
    </button>
  );

  return (
    <dialog
      ref={dialog}
      onClose={onClose}
      aria-label="Browse models"
      onClick={(event) => { if (event.target === event.currentTarget) dialog.current?.close(); }}
      className="m-auto w-[calc(100%_-_2rem)] max-w-3xl border-0 bg-transparent p-0 text-primary backdrop:bg-black/60"
    >
      <div className="card flex max-h-[80dvh] w-full flex-col overflow-hidden">
        <div className="flex items-start justify-between gap-4 px-5 pt-5">
          <div>
            <h2 className="text-base font-semibold text-primary">Models</h2>
            <p className="mt-0.5 text-xs text-muted">Every configured provider instance and the models it advertises.</p>
          </div>
          <button
            type="button"
            onClick={() => dialog.current?.close()}
            className="touch-target -mr-1.5 -mt-1.5 inline-flex items-center justify-center rounded-md p-1.5 text-muted hover:bg-white/5 hover:text-primary max-sm:min-h-11 max-sm:min-w-11"
            aria-label="Close model browser"
          >
            <X size={16} aria-hidden="true" />
          </button>
        </div>

        <div className="space-y-3 px-5 pt-4">
          <div className="relative">
            <Search size={15} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted" aria-hidden="true" />
            <input
              type="search"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              aria-label="Search models"
              placeholder="Search provider, model or id"
              className="w-full rounded-md border border-subtle bg-raised py-2.5 pl-9 pr-3 text-base text-primary placeholder:text-muted focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent sm:text-sm"
            />
          </div>
          <div className="flex flex-wrap items-center gap-1.5" role="group" aria-label="Filter models">
            {chip('all', 'All')}
            {chip('favorites', 'Favorites')}
            {backends.map((backend) => chip(backend.id, backend.displayName))}
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
          {loading ? (
            <ul className="grid gap-2 sm:grid-cols-2" aria-label="Loading models">
              {Array.from({ length: 6 }, (_, index) => (
                <li key={index} className="h-[4.5rem] animate-pulse rounded-lg border border-subtle bg-raised/60" />
              ))}
            </ul>
          ) : visible.length === 0 ? (
            <p className="py-10 text-center text-sm text-muted">
              {entries.length === 0 ? 'No providers are configured yet.' : 'No model matches that search.'}
            </p>
          ) : (
            <ul className="grid gap-2 sm:grid-cols-2">
              {visible.map((entry) => {
                const favorite = entryFavorite(entry);
                const saved = isFavorite(favorite);
                const enabled = entry.backend.implemented;
                const name = enabled
                  ? entry.model?.name ?? DEFAULT_MODEL_NAME
                  : `${entry.backend.displayName} (unavailable)`;
                const current = enabled && entry.backend.id === selectedBackendId
                  && (entry.model?.id ?? null) === selectedModelId;
                return (
                  <li key={`${entry.backend.id}\n${entry.model?.id ?? ''}`} className="relative">
                    <button
                      type="button"
                      onClick={() => { onPick(entry.backend.id, entry.model?.id ?? null); dialog.current?.close(); }}
                      disabled={!enabled}
                      aria-current={current ? 'true' : undefined}
                      title={enabled ? undefined : 'This provider is configured but not wired up yet'}
                      className={`h-full w-full rounded-lg border px-3 py-2.5 pr-14 text-left transition-colors ${
                        current
                          ? 'border-accent/50 bg-accent/10'
                          : enabled
                            ? 'border-subtle bg-raised hover:border-accent/30 hover:bg-white/5'
                            : 'border-subtle bg-raised/40 opacity-60'
                      }`}
                    >
                      <span className="block truncate text-sm font-medium text-primary">{name}</span>
                      {enabled && <span className="block truncate text-[11px] text-muted">{entry.backend.displayName}</span>}
                      {!enabled ? (
                        <span className="mt-1 block text-xs text-secondary">Configured, but this harness is not wired up yet.</span>
                      ) : entry.model?.description ? (
                        <span className="mt-1 line-clamp-2 block text-xs text-secondary">{entry.model.description}</span>
                      ) : entry.model ? (
                        <span className="mt-1 block truncate font-mono text-[10px] text-muted">{entry.model.id}</span>
                      ) : (
                        <span className="mt-1 block text-xs text-secondary">Whatever this instance is configured to use.</span>
                      )}
                    </button>
                    {enabled && <button
                      type="button"
                      onClick={() => onToggleFavorite(favorite)}
                      aria-pressed={saved}
                      aria-label={`Favorite ${name} on ${entry.backend.displayName}`}
                      className="touch-target absolute right-1.5 top-1.5 inline-flex items-center justify-center rounded-md p-1.5 text-muted hover:bg-white/5 hover:text-primary max-sm:min-h-11 max-sm:min-w-11"
                      title={saved ? 'Remove from favorites' : 'Add to favorites'}
                    >
                      <Star size={13} className={saved ? 'fill-amber-400 text-amber-400' : ''} aria-hidden="true" />
                    </button>}
                  </li>
                );
              })}
            </ul>
          )}
        </div>

        <div className="flex flex-wrap items-center justify-between gap-2 border-t border-subtle px-5 py-3">
          <p className="text-[11px] text-muted" role="status">
            {failed.length > 0
              ? `${failed.length} provider${failed.length === 1 ? '' : 's'} did not answer. Their defaults are still selectable.`
              : loading ? 'Loading models…' : `${visible.length} of ${entries.length} shown`}
          </p>
          {failed.length > 0 && (
            <button type="button" onClick={() => setRetryEpoch((value) => value + 1)} className="btn-secondary text-xs max-sm:min-h-11 max-sm:min-w-11">
              Retry
            </button>
          )}
        </div>
      </div>
    </dialog>
  );
}
