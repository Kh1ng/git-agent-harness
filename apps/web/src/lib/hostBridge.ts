/**
 * Host bridge: the single module that knows which native shell the
 * dashboard web app is running in and how each host services its
 * capabilities. The contract for hosts is docs/HOST_BRIDGE.md:
 *
 * - Desktop (Linux/Windows/macOS, Tauri): capabilities go through
 *   guarded `invoke` commands. The shell's navigation policy blocks all
 *   external navigation, so external links MUST use the
 *   `open_external_url` command, which validates the URL and opens the
 *   OS browser itself.
 * - iOS shell: new-window requests (`target="_blank"`) are discarded
 *   entirely and programmatic `location.href` navigation is not routed
 *   to the system browser -- only a real anchor click is. External
 *   links must therefore run as the browser's default anchor
 *   navigation, which the shell's navigation delegate opens via
 *   `UIApplication.open`.
 * - Android shell: same passthrough contract; the shell's
 *   `shouldOverrideUrlLoading` opens external origins in the system
 *   browser.
 * - Plain browser: external links open in a `noopener` tab.
 */

type DesktopHostWindow = Window & { __GAH_DESKTOP_EXTERNAL_LINKS__?: boolean };
type IosHostWindow = Window & { webkit?: { messageHandlers?: { gahController?: unknown } } };

type HostKind = 'browser' | 'desktop' | 'ios-shell' | 'android-shell';

function detectHost(): HostKind {
  if ((window as DesktopHostWindow).__GAH_DESKTOP_EXTERNAL_LINKS__ === true) return 'desktop';
  if ((window as IosHostWindow).webkit?.messageHandlers?.gahController) return 'ios-shell';
  if (typeof navigator !== 'undefined' && navigator.userAgent.includes('GAH-Android')) return 'android-shell';
  return 'browser';
}

export type ExternalOpenResult = 'handled' | 'passthrough';

/**
 * Opens an external HTTP(S) URL from a user's anchor click.
 *
 * Returns `'handled'` when the open is done and the caller must cancel
 * the default navigation, or `'passthrough'` when the caller must let
 * the default anchor navigation run because the mobile shells route a
 * real anchor click to the system browser. This result is part of the
 * bridge's internal contract with its anchor component, not host
 * knowledge for general callers.
 */
export function openExternal(href: string): ExternalOpenResult {
  switch (detectHost()) {
    case 'desktop': {
      void import('@tauri-apps/api/core')
        .then(({ invoke }) => invoke('open_external_url', { url: href }))
        .catch(() => {
          // Older desktop build without the command: degrade to the
          // plain-browser behavior instead of a dead link.
          window.open(href, '_blank', 'noopener,noreferrer');
        });
      return 'handled';
    }
    case 'ios-shell':
    case 'android-shell':
      return 'passthrough';
    default:
      window.open(href, '_blank', 'noopener,noreferrer');
      return 'handled';
  }
}
