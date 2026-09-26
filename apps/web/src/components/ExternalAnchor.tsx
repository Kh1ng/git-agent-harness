import { type AnchorHTMLAttributes, type MouseEvent } from 'react';
import { openExternal } from '../lib/hostBridge';

type ExternalAnchorProps = AnchorHTMLAttributes<HTMLAnchorElement>;

/**
 * An anchor to a provider page or other URL outside the dashboard.
 *
 * Renders a normal `<a>` that works in every host the dashboard runs in:
 * the desktop shells route it through their guarded open command, the
 * iOS and Android shells open the real anchor navigation in the system
 * browser, and a plain browser opens it in a `noopener` tab. Callers
 * never set `target="_blank"`: embedded hosts discard new-window
 * requests, which is why raw external anchors were dead links there.
 *
 * Modified clicks (cmd/ctrl/shift/alt, non-primary buttons) fall
 * through to the browser default.
 */
export function ExternalAnchor({ href, onClick, rel, ...rest }: ExternalAnchorProps) {
  return (
    <a
      href={href}
      rel={rel ?? 'noopener noreferrer'}
      onClick={(event: MouseEvent<HTMLAnchorElement>) => {
        onClick?.(event);
        if (!href || event.defaultPrevented) return;
        if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
        if (openExternal(href) === 'handled') event.preventDefault();
      }}
      {...rest}
    />
  );
}
