/// <reference types="vite/client" />

/** Injected by vite.config.ts `define` at build time from package.json + git HEAD. */
declare const __GAH_VERSION__: string;
declare const __GAH_COMMIT__: string;

interface Window {
  /** The desktop shell exposes navigation bridges; local commands stay in bundled Settings. */
  __GAH_DESKTOP_SETTINGS__?: boolean;
  __GAH_DESKTOP_NATIVE_NOTIFICATIONS__?: boolean;
  webkit?: { messageHandlers?: { gahController?: { postMessage: (message: unknown) => void } } };
}
