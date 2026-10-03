import type { MistralLoginResult } from './mistralLogin.js';

export type CredentialInfo = { id: string; provider: string; kind: 'api_key' | 'mistral_dashboard'; account_label: string; env_var: string | null };
type CredentialInstance = { profile: string; instance: string; runner_kind: string; credential_id: string | null };
type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

const usageOnly = (entry: CredentialInfo) => /_ADMIN_|_ADMINISTRATOR_/.test(entry.env_var ?? '');
function supportsRunner(entry: CredentialInfo, runner: string): boolean {
  if (usageOnly(entry)) return false;
  const fixedProvider = ({ codex: 'openai', claude: 'anthropic', vibe: 'mistral' } as Record<string, string>)[runner];
  return fixedProvider ? entry.provider === fixedProvider : ['opencode', 'hermes', 'openhands'].includes(runner);
}

/** Manages named credentials on this device. Only transient masked input goes
 * to native save; listing, status events and rendering contain metadata. */
export function bindProviderConnections(section: HTMLElement, invoke: Invoke) {
  const form = section.querySelector<HTMLFormElement>('#credential-form')!;
  const provider = section.querySelector<HTMLInputElement>('#credential-provider')!;
  const label = section.querySelector<HTMLInputElement>('#credential-label')!;
  const secret = section.querySelector<HTMLInputElement>('#credential-secret')!;
  const env = section.querySelector<HTMLInputElement>('#credential-env')!;
  const list = section.querySelector<HTMLUListElement>('#credential-list')!;
  const status = section.querySelector<HTMLElement>('#credential-status')!;
  const dashboardLabel = section.querySelector<HTMLInputElement>('#mistral-account-label')!;
  const dashboardConnect = section.querySelector<HTMLButtonElement>('#mistral-add-account')!;
  let entries: CredentialInfo[] = [];
  let instances: CredentialInstance[] = [];
  const pending = new Map<string, string>();
  const revisions = new Map<string, number>();
  const checks = new Map<string, string>();
  const enabled = new WeakMap<HTMLButtonElement, () => boolean>();
  let busy = false;
  const connectionNote = (entry: CredentialInfo) => `Saved on this computer${usageOnly(entry) ? ' · Usage access only' : ''}${checks.has(entry.id) ? ` · ${checks.get(entry.id)}` : ''}`;

  function button(text: string, action: () => Promise<void>, available = () => true) {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = text;
    enabled.set(button, available);
    button.disabled = busy || !available();
    button.addEventListener('click', () => { void perform(action); });
    return button;
  }

  function render() {
    list.replaceChildren();
    for (const entry of entries) {
      const row = document.createElement('li');
      row.dataset.credentialId = entry.id;
      const heading = document.createElement('p');
      heading.textContent = `${entry.account_label} · ${entry.provider} · ${entry.kind === 'mistral_dashboard' ? 'Mistral sign-in' : 'API key'}`;
      const note = document.createElement('small');
      note.textContent = connectionNote(entry);
      row.append(heading, note);
      const actions = document.createElement('div');
      actions.className = 'actions';
      actions.append(button('Check usage', async () => {
        try {
          await invoke('credential_refresh', { id: entry.id });
          checks.set(entry.id, 'Usage check completed');
          note.textContent = connectionNote(entry);
          status.textContent = `Usage check completed for ${entry.account_label}. See Quota management for available readings.`;
        } catch {
          checks.set(entry.id, 'Usage check failed');
          note.textContent = connectionNote(entry);
          status.textContent = `Usage is unavailable for ${entry.account_label}. This key remains saved; some providers require a separate account sign-in for usage.`;
        }
      }));
      if (entry.kind === 'mistral_dashboard') {
        actions.append(button('Reconnect', () => startLogin(entry.id, entry.account_label)));
      } else {
        if (!usageOnly(entry)) {
          actions.append(button('Create local instance', async () => {
            const create = document.createElement('div');
            create.className = 'instance-create';
            const title = document.createElement('p');
            title.textContent = `Create an instance using ${entry.account_label} on this computer`;
            create.append(title);
            const field = (text: string, input: HTMLElement) => {
              const label = document.createElement('label');
              label.append(text, input);
              create.append(label);
            };
            const profile = document.createElement('select');
            profile.setAttribute('aria-label', 'Local profile');
            const profiles = [...new Set(instances.map(instance => instance.profile))];
            for (const name of profiles.length ? profiles : ['gah']) {
              const option = document.createElement('option');
              option.value = option.textContent = name;
              profile.append(option);
            }
            field('Profile', profile);
            const runner = document.createElement('select');
            runner.setAttribute('aria-label', 'Local runner');
            for (const [value, name] of [['codex', 'Codex'], ['claude', 'Claude'], ['vibe', 'Vibe'], ['opencode', 'OpenCode'], ['hermes', 'Hermes'], ['openhands', 'OpenHands']]) {
              if (!supportsRunner(entry, value)) continue;
              const option = document.createElement('option');
              option.value = value;
              option.textContent = name;
              runner.append(option);
            }
            runner.value = ({ mistral: 'vibe', nous: 'opencode', anthropic: 'claude', openai: 'codex' } as Record<string, string>)[entry.provider] ?? 'opencode';
            field('Runner', runner);
            const instance = document.createElement('input');
            instance.setAttribute('aria-label', 'Local instance ID');
            instance.maxLength = 64;
            instance.autocomplete = 'off';
            instance.spellcheck = false;
            instance.placeholder = `${runner.value}-work`;
            field('Instance ID', instance);
            const createButton = button('Create and use key', async () => {
              if (!instance.value.trim()) return;
              await invoke('credential_add_instance', { profile: profile.value, instance: instance.value.trim(), runnerKind: runner.value, credentialId: entry.id });
              await refresh();
              status.textContent = `${profile.value} / ${instance.value.trim()} was created using ${entry.account_label} on this computer.`;
            }, () => !!instance.value.trim());
            instance.addEventListener('input', () => { createButton.disabled = busy || !instance.value.trim(); });
            create.append(createButton, button('Cancel instance creation', async () => { create.remove(); }));
            row.querySelector('.instance-create')?.remove();
            row.append(create);
            instance.focus();
          }));
          const binding = document.createElement('div');
          binding.className = 'actions';
          const select = document.createElement('select');
          select.setAttribute('aria-label', `Local instance for ${entry.account_label}`);
          const prompt = document.createElement('option');
          prompt.value = '';
          prompt.textContent = 'Select an existing local instance';
          select.append(prompt);
          for (const instance of instances.filter(instance => supportsRunner(entry, instance.runner_kind))) {
            const option = document.createElement('option');
            option.value = JSON.stringify([instance.profile, instance.instance]);
            option.textContent = `${instance.profile} / ${instance.instance} · ${instance.runner_kind}${instance.credential_id === entry.id ? ' · using this key' : ''}`;
            select.append(option);
          }
          const use = button('Use on instance', async () => {
            if (!select.value) return;
            const [profile, instance] = JSON.parse(select.value) as [string, string];
            await invoke('credential_bind', { profile, instance, credentialId: entry.id });
            await refresh();
            status.textContent = `${entry.account_label} is selected for ${profile} / ${instance} on this computer.`;
          }, () => !!select.value);
          select.addEventListener('change', () => { use.disabled = busy || !select.value; });
          binding.append(select, use);
          row.append(binding);
        }
        actions.append(button('Replace key', async () => {
          form.hidden = false;
          form.dataset.editId = entry.id;
          provider.value = entry.provider;
          label.value = entry.account_label;
          env.value = entry.env_var ?? '';
          secret.value = '';
          secret.focus();
        }));
      }
      actions.append(button('Remove', async () => {
        const confirm = document.createElement('div');
        confirm.className = 'actions';
        const message = document.createElement('span');
        const bound = instances.filter(instance => instance.credential_id === entry.id).length;
        message.textContent = `Remove saved connection ${entry.account_label}?${bound ? ` ${bound} local ${bound === 1 ? 'instance uses' : 'instances use'} it and will need another credential.` : ''}`;
        confirm.append(message, button('Remove connection', async () => {
          await invoke('credential_remove', { id: entry.id });
          await refresh();
          status.textContent = `${entry.account_label} was removed from this computer.`;
        }), button('Keep connection', async () => { confirm.remove(); }));
        row.append(confirm);
      }));
      row.append(actions);
      list.append(row);
    }
    for (const [id, name] of pending) {
      const row = document.createElement('li');
      const note = document.createElement('p');
      note.textContent = `${name} · Mistral sign-in in progress`;
      row.append(note, button('Check connection', async () => {
        await login('mistral_login_finish', id, name);
      }));
      list.append(row);
    }
  }

  async function refresh() {
    const results = await Promise.allSettled([
      invoke<CredentialInfo[]>('credential_list'),
      invoke<CredentialInstance[]>('credential_instances'),
    ]);
    if (results[0].status === 'rejected') throw results[0].reason;
    entries = results[0].value;
    instances = results[1].status === 'fulfilled' ? results[1].value : [];
    if (results[1].status === 'rejected') status.textContent = 'Saved connections are available, but local instances could not be checked. Update GAH and retry.';
    else status.textContent = entries.length ? `${entries.length} named connections saved on this computer.` : 'No named connections saved yet. Your existing default Mistral connection stays above.';
    render();
  }

  async function perform(action: () => Promise<void>) {
    if (busy) return;
    busy = true;
    section.querySelectorAll('button').forEach(button => { button.disabled = true; });
    try { await action(); } catch {
      status.textContent = 'The connection could not be updated. Check the fields and update GAH on this computer.';
    } finally {
      busy = false;
      section.querySelectorAll('button').forEach(button => { button.disabled = !(enabled.get(button)?.() ?? true); });
    }
  }

  async function startLogin(id: string, name: string) {
    checks.delete(id);
    pending.set(id, name);
    render();
    await login('mistral_login_start', id, name);
  }

  async function login(command: string, id: string, name: string) {
    const revision = revisions.get(id) ?? 0;
    try {
      const result = await invoke<MistralLoginResult>(command, { credentialId: id, accountLabel: name });
      if ((revisions.get(id) ?? 0) === revision) show(result);
    } catch (error) {
      pending.delete(id);
      render();
      throw error;
    }
  }

  function show(result: MistralLoginResult) {
    if (!result.credential_id) return;
    revisions.set(result.credential_id, (revisions.get(result.credential_id) ?? 0) + 1);
    status.textContent = result.message;
    if (result.state === 'connected' || result.state === 'cancelled' || result.state === 'unavailable' && !result.installed) {
      pending.delete(result.credential_id);
      void refresh().catch(() => { status.textContent = 'Connection status is unavailable. Select Check saved connections to retry.'; });
    }
    render();
  }

  section.querySelector('#credential-add')!.addEventListener('click', () => {
    form.reset();
    delete form.dataset.editId;
    form.hidden = false;
    label.focus();
  });
  section.querySelector('#credential-cancel')!.addEventListener('click', () => {
    secret.value = '';
    form.hidden = true;
  });
  section.querySelector('#credential-reload')!.addEventListener('click', () => { void perform(refresh); });
  form.addEventListener('submit', event => {
    event.preventDefault();
    const key = secret.value;
    const id = form.dataset.editId ?? `key-${crypto.randomUUID()}`;
    secret.value = '';
    void perform(async () => {
      await invoke<CredentialInfo>('credential_save', { input: {
        id,
        provider: provider.value.trim(), kind: 'api_key', account_label: label.value.trim(),
        env_var: env.value.trim() || null, secret: key,
      } });
      checks.delete(id);
      form.reset();
      form.hidden = true;
      await refresh();
      status.textContent = 'Key saved privately on this computer. Usage has not been checked.';
    });
  });
  dashboardConnect.addEventListener('click', () => {
    const name = dashboardLabel.value.trim();
    if (!name) { dashboardLabel.reportValidity(); return; }
    void perform(() => startLogin(`mistral-${crypto.randomUUID()}`, name));
  });
  void perform(refresh);
  return show;
}
