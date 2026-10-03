import { useEffect, useRef } from 'react';
import settingsHtml from '../../../../desktop/index.html?raw';
import { bindProviderConnections, type CredentialInfo } from '../../../../desktop/src/providerConnections.js';

export function NativeProviderConnections({ entries: initialEntries, failUsage = false, failReload = false, automatic = false }: { entries: CredentialInfo[]; failUsage?: boolean; failReload?: boolean; automatic?: boolean }) {
  const root = useRef<HTMLDivElement>(null);
  const section = settingsHtml.match(/<section id="provider-connections"[\s\S]*?<\/section>/)?.[0] ?? '';
  const style = settingsHtml.match(/<style>[\s\S]*?<\/style>/)?.[0] ?? '';
  useEffect(() => {
    let entries = [...initialEntries];
    let lists = 0;
    const calls: { command: string; id?: string; secretMatched?: boolean }[] = [];
    const show = bindProviderConnections(root.current!.querySelector<HTMLElement>('#provider-connections')!, async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
      const call: { command: string; id?: string; secretMatched?: boolean } = { command };
      let result: unknown;
      if (command === 'credential_list') {
        if (failReload && lists++ > 0) throw new Error('private native error');
        result = entries;
      } else if (command === 'credential_instances') {
        result = [
          { profile: 'local-work', instance: 'vibe-work', runner_kind: 'vibe', credential_id: null },
          { profile: 'local-work', instance: 'agy-one', runner_kind: 'agy', credential_id: null },
        ];
      } else if (command === 'credential_save') {
        const input = args!.input as CredentialInfo & { secret: string };
        call.id = input.id;
        call.secretMatched = input.secret === 'synthetic-private-api-key';
        const { secret: _secret, ...metadata } = input;
        entries = [...entries.filter(entry => entry.id !== metadata.id), metadata];
        result = metadata;
      } else if (command === 'credential_remove') {
        call.id = String(args!.id);
        entries = entries.filter(entry => entry.id !== args!.id);
      } else if (command === 'credential_bind' || command === 'credential_add_instance') {
        call.id = String(args!.credentialId);
      } else if (command === 'credential_refresh') {
        if (failUsage) throw new Error('synthetic-private-api-key must not reach DOM');
      } else if (command === 'mistral_login_start' || command === 'mistral_login_finish') {
        const id = String(args!.credentialId);
        call.id = id;
        const metadata: CredentialInfo = { id, provider: 'mistral', kind: 'mistral_dashboard', account_label: String(args!.accountLabel), env_var: null };
        const connected = command === 'mistral_login_finish' || automatic;
        if (connected) entries = [...entries.filter(entry => entry.id !== id), metadata];
        if (automatic) show({ state: 'connected', credential_id: id, installed: true, message: 'Connected on this computer.' });
        result = { state: command === 'mistral_login_finish' ? 'connected' : 'pending', credential_id: id, installed: true, message: command === 'mistral_login_finish' ? 'Connected on this computer.' : 'Sign in to this account.' };
      }
      calls.push(call);
      root.current!.querySelector('#native-calls')!.textContent = JSON.stringify(calls);
      return result as T;
    });
  }, [initialEntries, failUsage, failReload, automatic]);
  return <div ref={root} style={{ backgroundColor: '#0b0e14', color: '#e6e9ef', minHeight: '100vh' }}><div dangerouslySetInnerHTML={{ __html: `${style}<main>${section}</main>` }} /><output id="native-calls" hidden /></div>;
}
