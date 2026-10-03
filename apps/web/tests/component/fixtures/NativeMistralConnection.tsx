import { useEffect, useRef } from 'react';
import settingsHtml from '../../../../desktop/index.html?raw';
import { bindMistralLogin, type MistralLoginResult } from '../../../../desktop/src/mistralLogin.js';

/** Exercises the real bundled Settings markup and its native command boundary. */
export function NativeMistralConnection({ responses, automatic }: { responses: Array<MistralLoginResult | null>; automatic?: MistralLoginResult }) {
  const root = useRef<HTMLDivElement>(null);
  const html = settingsHtml.match(/<section id="mistral-section"[\s\S]*?<\/section>/)?.[0] ?? '';
  const style = settingsHtml.match(/<style>[\s\S]*?<\/style>/)?.[0] ?? '';
  useEffect(() => {
    let index = 0;
    const show = bindMistralLogin(root.current!.querySelector<HTMLElement>('#mistral-section')!, async command => {
      const item = document.createElement('li');
      item.textContent = command;
      root.current!.querySelector('#commands')!.append(item);
      const response = responses[index++];
      if (automatic) {
        show(automatic);
        await Promise.resolve();
      }
      if (!response) throw new Error('Private native diagnostic must not be rendered');
      return response;
    });
  }, [responses, automatic]);
  return <div ref={root} style={{ backgroundColor: '#0b0e14', color: '#e6e9ef', minHeight: '100vh' }}><div dangerouslySetInnerHTML={{ __html: `${style}<main>${html}</main>` }} /><ol id="commands" aria-label="Native commands" /></div>;
}
