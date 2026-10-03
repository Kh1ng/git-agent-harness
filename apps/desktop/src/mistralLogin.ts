export type MistralLoginResult = {
  state: 'pending' | 'connected' | 'cancelled' | 'unavailable';
  installed: boolean;
  message: string;
};

/** Connects this device through the native sign-in window; credentials never enter this page. */
export function bindMistralLogin(section: HTMLElement, invoke: (command: string) => Promise<MistralLoginResult>) {
  const start = section.querySelector<HTMLButtonElement>('#mistral-connect')!;
  const finish = section.querySelector<HTMLButtonElement>('#mistral-finish')!;
  const status = section.querySelector<HTMLElement>('#mistral-status')!;
  const installation = section.querySelector<HTMLElement>('#mistral-installation')!;
  let busy = false;
  let revision = 0;

  function show(result: MistralLoginResult) {
    revision++;
    status.textContent = result.message;
    installation.hidden = result.installed;
    finish.hidden = result.state !== 'pending' || !result.installed;
    start.textContent = result.state === 'connected' ? 'Reconnect Mistral' : 'Connect Mistral';
  }

  async function run(command: string) {
    if (busy) return;
    busy = true;
    const startedAt = revision;
    start.disabled = finish.disabled = true;
    status.textContent = command === 'mistral_login_start' ? 'Opening Mistral sign-in…' : 'Checking this computer’s connection…';
    try {
      const result = await invoke(command);
      // A verified native event can arrive before the start/check command resolves.
      if (revision === startedAt) show(result);
    } catch {
      if (revision === startedAt) status.textContent = 'Could not connect Mistral. Try again from this computer’s Settings.';
    } finally {
      busy = false;
      start.disabled = finish.disabled = false;
    }
  }

  start.addEventListener('click', () => { void run('mistral_login_start'); });
  finish.addEventListener('click', () => { void run('mistral_login_finish'); });
  return show;
}
