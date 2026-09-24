import { useEffect, useRef, useState } from 'react';
import { ChevronDown, FolderOpen } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';

interface OpenTool {
  id: string;
  label: string;
}

interface OpenContext {
  available: boolean;
  preferredTool: string | null;
  tools: OpenTool[];
  reason: string | null;
}

interface OpenProjectRef {
  profile: string;
  nodeId?: string;
  sessionId?: string;
}

export function OpenLocalCheckout({ profile, nodeId, nodeName, sessionId }: OpenProjectRef & { nodeName: string }) {
  const [context, setContext] = useState<OpenContext | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const menuButton = useRef<HTMLButtonElement>(null);
  const project = { profile, ...(nodeId ? { nodeId } : {}), ...(sessionId ? { sessionId } : {}) };

  useEffect(() => {
    if (window.__GAH_DESKTOP_OPEN_PROJECT__ !== true) return;
    let cancelled = false;
    setContext(null);
    invoke<OpenContext>('desktop_open_context', { project })
      .then(result => { if (!cancelled) setContext(result); })
      .catch(() => { if (!cancelled) setContext({ available: false, preferredTool: null, tools: [], reason: 'This checkout is not available on this device.' }); });
    return () => { cancelled = true; };
  }, [profile, nodeId, sessionId]);

  useEffect(() => {
    if (!open) return;
    const closeOutside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    const closeEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { setOpen(false); menuButton.current?.focus(); }
    };
    document.addEventListener('pointerdown', closeOutside);
    document.addEventListener('keydown', closeEscape);
    return () => {
      document.removeEventListener('pointerdown', closeOutside);
      document.removeEventListener('keydown', closeEscape);
    };
  }, [open]);

  if (window.__GAH_DESKTOP_OPEN_PROJECT__ !== true || !context) return null;
  if (!context.available) {
    return <span className="text-xs text-muted" title={context.reason ?? undefined}>Files on {nodeName}</span>;
  }
  const preferred = context.tools.find(tool => tool.id === context.preferredTool) ?? context.tools[0];
  if (!preferred) return null;

  const launch = async (tool: OpenTool, fromMenu = false) => {
    if (busy) return;
    if (fromMenu) {
      setOpen(false);
    }
    setBusy(true);
    setError(null);
    try {
      await invoke('open_local_checkout', { project, tool: tool.id });
      setContext(current => current ? { ...current, preferredTool: tool.id } : current);
      setOpen(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
      if (fromMenu) requestAnimationFrame(() => menuButton.current?.focus());
    }
  };

  return (
    <div ref={root} className="relative flex items-center">
      <button type="button" className="btn-secondary rounded-r-none border-r-0 text-xs" disabled={busy}
        onClick={() => void launch(preferred)} title={`Open this checkout in ${preferred.label}`}>
        <FolderOpen size={13} /> {busy ? 'Opening…' : `Open in ${preferred.label}`}
      </button>
      <button ref={menuButton} type="button" className="btn-secondary rounded-l-none px-2 text-xs" disabled={busy}
        aria-label="Choose local app" aria-expanded={open} onClick={() => setOpen(value => !value)}>
        <ChevronDown size={13} />
      </button>
      {open && (
        <div className="absolute right-0 top-full z-30 mt-1 min-w-48 rounded-md border border-subtle bg-raised p-1 shadow-lg">
          {context.tools.map(tool => (
            <button key={tool.id} type="button" className="chat-menu-item text-xs"
              onClick={() => void launch(tool, true)}>{tool.label}{tool.id === context.preferredTool ? <span className="ml-auto text-muted">Last used</span> : null}</button>
          ))}
        </div>
      )}
      {error && <span role="alert" className="absolute right-0 top-full z-20 mt-1 w-64 rounded-md border border-critical/40 bg-card px-3 py-2 text-xs text-critical">{error}</span>}
    </div>
  );
}
