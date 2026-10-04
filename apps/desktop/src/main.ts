import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { bindMistralLogin, type MistralLoginResult } from './mistralLogin.js';
import { bindProviderConnections } from './providerConnections.js';

type Presence = { dock: boolean; launch_window: boolean; tray: boolean };
type Settings = { central_url: string; wsl_distribution: string; presence: Presence };
type WorkerStatus = { running: boolean; note: string; tools: { name: string; environment: string; installed: boolean }[] };
type RoleStatus = { role: 'central' | 'worker'; running: boolean; supported: boolean };
const central = document.querySelector<HTMLInputElement>('#central-url')!;
const distribution = document.querySelector<HTMLInputElement>('#wsl-distribution')!;
const error = document.querySelector<HTMLElement>('#error')!;
const status = document.querySelector<HTMLElement>('#status')!;
const dock = document.querySelector<HTMLInputElement>('#show-dock')!;
const launchWindow = document.querySelector<HTMLInputElement>('#launch-window')!;
const tray = document.querySelector<HTMLInputElement>('#show-tray')!;
const ownerToken = document.querySelector<HTMLInputElement>('#owner-token')!;
const ownerState = document.querySelector<HTMLElement>('#owner-state')!;
const isMac = navigator.userAgent.includes('Mac');
const showMistralLogin = bindMistralLogin(document.querySelector<HTMLElement>('#mistral-section')!, command => invoke<MistralLoginResult>(command));
const showProviderConnection = bindProviderConnections(document.querySelector<HTMLElement>('#provider-connections')!, invoke);
// Explicit Check connection remains available if automatic status events cannot be delivered.
void listen<MistralLoginResult>('gah:mistral-login', event => {
  showMistralLogin(event.payload);
  showProviderConnection(event.payload);
}).catch(() => {});

function showPresence(presence: Presence) {
  dock.checked = presence.dock;
  tray.checked = presence.tray;
  launchWindow.checked = presence.launch_window;
  constrainPresence();
}

function constrainPresence() {
  const windowRequired = !tray.checked && !(isMac && dock.checked);
  launchWindow.disabled = windowRequired;
  if (windowRequired) launchWindow.checked = true;
  document.querySelector<HTMLElement>('#presence-recovery')!.hidden = !windowRequired;
}

function showRole(result: RoleStatus) {
  document.querySelector<HTMLElement>('#role-section')!.hidden = !result.supported;
  document.querySelector('#role-state')!.textContent = `${result.role === 'central' ? 'Central' : 'Worker'} mode · ${result.running ? 'running' : 'stopped'}.`;
  for (const role of ['central', 'worker'] as const) {
    document.querySelector(`#${role}-role`)!.setAttribute('aria-pressed', String(role === result.role));
  }
}


type SetupStatus = { state: 'ok' | 'missing' | 'outdated' | 'not_logged_in' | 'unsupported'; found?: string; reason?: string };
type SetupRequirement = { id: string; label: string; why: string; optional: boolean; status: SetupStatus; action: { command: string; sudo: boolean } | null };
type SetupCheck = {
  installed: boolean;
  report: { ready: boolean; requirements: SetupRequirement[] } | null;
  error: string | null;
  command: string;
  terminal: boolean;
};
let nodeRole: 'central' | 'worker' = 'central';
let standaloneStarted = false;

function statusText(status: SetupStatus): string {
  switch (status.state) {
    case 'ok': return status.found ? `found ${status.found}` : 'ready';
    case 'missing': return 'missing';
    case 'outdated': return `too old (found ${status.found})`;
    case 'not_logged_in': return 'not logged in';
    case 'unsupported': return status.reason ?? 'not supported here';
  }
}

/** `gah setup --check` as a checklist; the work itself happens in Terminal. Resolves to readiness. */
async function refreshSetup(): Promise<boolean> {
  const result = await invoke<SetupCheck>('setup_check', { role: nodeRole });
  const state = document.querySelector<HTMLElement>('#setup-state')!;
  const list = document.querySelector<HTMLElement>('#setup-list')!;
  const button = document.querySelector<HTMLButtonElement>('#setup-terminal')!;
  const commandLine = document.querySelector<HTMLElement>('#setup-command')!;
  list.replaceChildren();
  const pending = !result.installed || !result.report?.ready;
  if (!result.installed) {
    state.textContent = 'GAH is not installed on this computer yet. Setup builds it, asks whether this computer is the central node, a worker, or command line only, and offers each missing tool before installing it.';
    button.textContent = 'Install GAH in Terminal';
  } else if (result.error || !result.report) {
    state.textContent = result.error ?? 'Setup could not check this computer.';
    button.textContent = 'Finish setup in Terminal';
  } else {
    const missing = result.report.requirements.filter((item) => item.status.state !== 'ok' && !item.optional).length;
    state.textContent = result.report.ready
      ? 'Everything this computer needs is in place.'
      : `${missing} required ${missing === 1 ? 'item is' : 'items are'} missing. Setup offers each one before installing it.`;
    button.textContent = 'Finish setup in Terminal';
    for (const item of result.report.requirements) {
      const ok = item.status.state === 'ok';
      const row = document.createElement('li');
      const mark = document.createElement('span');
      mark.className = `mark ${ok ? 'ok' : item.optional ? 'optional' : 'missing'}`;
      mark.textContent = ok ? '✓' : item.optional ? '·' : '✗';
      mark.setAttribute('aria-label', ok ? 'Ready' : item.optional ? 'Optional' : 'Missing');
      row.append(mark, `${item.label}: ${statusText(item.status)}${item.optional && !ok ? ' (optional)' : ''}`);
      const why = document.createElement('small');
      why.textContent = item.why;
      row.append(why);
      if (!ok && item.action) {
        const how = document.createElement('small');
        const code = document.createElement('code');
        code.textContent = item.action.command;
        how.append(code, item.action.sudo ? ' (asks for your password)' : '');
        row.append(how);
      }
      list.append(row);
    }
  }
  list.hidden = list.childElementCount === 0;
  button.hidden = !pending || !result.terminal;
  document.querySelector<HTMLElement>('#setup-standalone')!.hidden = button.hidden;
  document.querySelector<HTMLElement>("#setup-factory-preference")!.hidden = button.hidden;
  document.querySelector<HTMLElement>('#standalone-note')!.hidden = button.hidden;
  commandLine.hidden = !pending || result.terminal;
  commandLine.querySelector('code')!.textContent = result.command;
  return !pending;
}

async function perform(action: () => Promise<void>) {
  error.textContent = '';
  const buttons = [...document.querySelectorAll('button')].filter(button => !button.closest('#provider-connections, #mistral-section'));
  buttons.forEach((button) => { button.disabled = true; });
  try { await action(); } catch (err) { error.textContent = String(err); }
  finally { buttons.forEach((button) => { button.disabled = false; }); }
}

async function refresh() {
  const result = await invoke<WorkerStatus>('worker_status');
  document.querySelector('#worker-state')!.textContent = result.running ? 'Worker process is running. Check profile readiness in the dashboard before dispatching.' : 'Worker is stopped or has not been installed.';
  document.querySelector('#worker-note')!.textContent = result.note;
  const body = document.querySelector('#tools tbody')!;
  body.replaceChildren();
  for (const tool of result.tools) {
    const row = document.createElement('tr');
    for (const value of [tool.name, tool.environment, tool.installed ? 'Yes · login unchecked' : 'Not found']) {
      const cell = document.createElement('td');
      cell.textContent = value;
      row.append(cell);
    }
    body.append(row);
  }
  document.querySelector<HTMLElement>('#tools')!.hidden = false;
}

async function connect() {
  await invoke('connect_dashboard', { settings: { central_url: central.value.trim(), wsl_distribution: distribution.value.trim() } });
}

async function refreshOwnerCredential() {
  if (!central.value.trim()) {
    ownerState.textContent = 'Save a central node address first.';
    return;
  }
  const saved = await invoke<boolean>('owner_credential_status', { origin: central.value.trim() });
  ownerState.textContent = saved
    ? 'Owner access is saved in this computer\'s credential vault.'
    : 'No owner token is saved for this central origin.';
}

document.querySelector('#back')!.addEventListener('click', () => {
  void perform(async () => { await invoke('open_central_settings'); });
});

document.querySelector('#connection')!.addEventListener('submit', (event) => {
  event.preventDefault();
  void perform(async () => {
    await connect();
    status.textContent = 'Connecting… Use Settings in the app menu if central is unavailable.';
  });
});
document.querySelector('#owner-credential')!.addEventListener('submit', (event) => {
  event.preventDefault();
  void perform(async () => {
    await invoke('save_owner_credential', { origin: central.value.trim(), token: ownerToken.value });
    ownerToken.value = '';
    await connect();
  });
});
document.querySelector('#forget-owner')!.addEventListener('click', () => {
  void perform(async () => {
    await invoke('forget_owner_credential', { origin: central.value.trim() });
    ownerToken.value = '';
    await connect();
  });
});
document.querySelector('#presence')!.addEventListener('submit', (event) => {
  event.preventDefault();
  void perform(async () => {
    const presence = await invoke<Presence>('save_presence', {
      presence: { dock: dock.checked, launch_window: launchWindow.checked, tray: tray.checked },
    });
    showPresence(presence);
    document.querySelector('#presence-status')!.textContent = 'Saved. Icon changes apply now; the launch preference applies next time you open GAH.';
  });
});
dock.addEventListener('change', constrainPresence);
tray.addEventListener('change', constrainPresence);
document.querySelector('#refresh')!.addEventListener('click', () => { void perform(refresh); });
document.querySelector('#setup-refresh')!.addEventListener('click', () => {
  void perform(async () => {
    if (await refreshSetup() && standaloneStarted) {
      standaloneStarted = false;
      await connect();
    }
  });
});
document.querySelector('#setup-standalone')!.addEventListener('click', () => {
  void perform(async () => {
    const factory = document.querySelector<HTMLInputElement>('#setup-factory-module')!.checked;
    central.value = await invoke<string>('open_setup_terminal', { standalone: true, factory });
    nodeRole = 'central';
    standaloneStarted = true;
    document.querySelector('#setup-state')!.textContent = 'Standalone setup is running in Terminal. Select Check again when it finishes to open the dashboard.';
  });
});
document.querySelector('#setup-terminal')!.addEventListener('click', () => {
  void perform(async () => {
    await invoke('open_setup_terminal');
    document.querySelector('#setup-state')!.textContent = 'Setup is running in Terminal. Check again when it finishes.';
  });
});
for (const [id, running] of [['start', true], ['stop', false]] as const) {
  document.querySelector(`#${id}`)!.addEventListener('click', () => {
    void perform(async () => { await invoke('set_worker_running', { running }); await refresh(); });
  });
}
for (const role of ['central', 'worker'] as const) {
  document.querySelector(`#${role}-role`)!.addEventListener('click', () => {
    status.textContent = `Updating this Mac for ${role} mode… This can take several minutes.`;
    void perform(async () => { showRole(await invoke<RoleStatus>('set_node_role', { role })); });
  });
}
void perform(async () => {
  const settings = await invoke<Settings>('desktop_settings');
  central.value = settings.central_url;
  await refreshOwnerCredential();
  document.querySelector<HTMLButtonElement>('#back')!.hidden = !settings.central_url;
  distribution.value = settings.wsl_distribution;
  document.querySelector<HTMLElement>('#dock-label')!.hidden = !isMac;
  showPresence(settings.presence);
  if (navigator.userAgent.includes('Windows')) {
    distribution.hidden = false;
    document.querySelector<HTMLElement>('#wsl-label')!.hidden = false;
  }
  const role = await invoke<RoleStatus>('node_role_status');
  showRole(role);
  nodeRole = role.role;
  // On Windows, setup and gah live in the selected WSL distribution.
  await refreshSetup();
});
