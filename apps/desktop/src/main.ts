import { bindRepositoryTools } from './repositoryTools.js';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { bindMistralLogin, type MistralLoginResult } from './mistralLogin.js';
import { bindProviderConnections } from './providerConnections.js';

type Presence = { dock: boolean; launch_window: boolean; tray: boolean };
type Settings = { central_url: string; wsl_distribution: string; presence: Presence };
type WorkerStatus = { running: boolean; note: string; tools: { name: string; environment: string; installed: boolean }[] };
type RoleStatus = { role: 'central' | 'worker'; running: boolean; supported: boolean };
type DesktopUpdateStatus = { available: boolean; currentVersion: string; version: string | null; notes: string | null; error: string | null };

/** Issue #1416: the signed release feed's verdict, shown as a non-blocking
 * Settings row. A missing signing key or unreachable feed reads as "not
 * configured" -- never an error dialog. */
function showDesktopUpdate(status: DesktopUpdateStatus): void {
  const text = document.querySelector<HTMLElement>('#desktop-update-status')!;
  const apply = document.querySelector<HTMLButtonElement>('#desktop-update-apply')!;
  if (status.available && status.version) {
    text.textContent = `Update available · v${status.currentVersion} → v${status.version} — Restart to update.`;
    apply.hidden = false;
  } else if (status.error) {
    text.textContent = status.error.includes('not configured')
      ? 'Automatic updates are not configured on this computer; the app still updates via setup.'
      : `Cannot check for updates: ${status.error}`;
    apply.hidden = true;
  } else {
    text.textContent = `This app is up to date (v${status.currentVersion}).`;
    apply.hidden = true;
  }
}

async function loadDesktopUpdate(): Promise<void> {
  showDesktopUpdate(await invoke<DesktopUpdateStatus>('desktop_update_status'));
}

document.querySelector('#desktop-update-apply')!.addEventListener('click', () => {
  const apply = document.querySelector<HTMLButtonElement>('#desktop-update-apply')!;
  apply.disabled = true;
  document.querySelector<HTMLElement>('#desktop-update-status')!.textContent = 'Downloading and installing the update…';
  void invoke('desktop_apply_update').catch((error: unknown) => {
    apply.disabled = false;
    document.querySelector<HTMLElement>('#desktop-update-status')!.textContent =
      `The update failed: ${error instanceof Error ? error.message : String(error)}`;
  });
});
bindRepositoryTools(document.querySelector<HTMLElement>('#repository-tools')!, invoke);
const central = document.querySelector<HTMLInputElement>('#central-url')!;
const distribution = document.querySelector<HTMLInputElement>('#wsl-distribution')!;
const error = document.querySelector<HTMLElement>('#error')!;
const status = document.querySelector<HTMLElement>('#status')!;
const dock = document.querySelector<HTMLInputElement>('#show-dock')!;
const launchWindow = document.querySelector<HTMLInputElement>('#launch-window')!;
const tray = document.querySelector<HTMLInputElement>('#show-tray')!;
const ownerToken = document.querySelector<HTMLInputElement>('#owner-token')!;
const ownerState = document.querySelector<HTMLElement>('#owner-state')!;
const nativeOnboarding = !navigator.userAgent.includes('Mac') && !navigator.userAgent.includes('Windows');
const isMac = navigator.userAgent.includes('Mac');
const showMistralLogin = bindMistralLogin(document.querySelector<HTMLElement>('#mistral-section')!, command => invoke<MistralLoginResult>(command));
const showProviderConnection = bindProviderConnections(document.querySelector<HTMLElement>('#provider-connections')!, invoke);
// Explicit Check connection remains available if automatic status events cannot be delivered.
void listen<MistralLoginResult>('gah:mistral-login', event => {
  showMistralLogin(event.payload);
  showProviderConnection(event.payload);
}).catch(() => {});
// The launch and periodic update checks (issue #1416) push the same status
// the Settings row loads on demand.
void listen<DesktopUpdateStatus>('gah:desktop-update', event => {
  showDesktopUpdate(event.payload);
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


type SetupStatus = { state: 'ok' | 'missing' | 'outdated' | 'not_logged_in' | 'credentials_rejected' | 'status_unknown' | 'status_failed' | 'unsupported'; found?: string; reason?: string };
type SetupRequirement = { id: string; label: string; why: string; optional: boolean; status: SetupStatus; action: { command: string; sudo: boolean } | null };
type SetupCheck = {
  installed: boolean;
  report: { ready: boolean; requirements: SetupRequirement[] } | null;
  error: string | null;
  command: string;
  terminal: boolean;
};
let nodeRole: 'central' | 'worker' = 'central';
function setupChoices() {
  return {
    role: document.querySelector<HTMLSelectElement>('#setup-mode')!.value,
    agent: document.querySelector<HTMLSelectElement>('#setup-agent')!.value,
    provider: document.querySelector<HTMLSelectElement>('#repository-provider')!.value === 'gh' ? 'github' : 'gitlab',
    memory: document.querySelector<HTMLSelectElement>('#setup-memory')!.value,
    gateway_url: document.querySelector<HTMLInputElement>('#setup-gateway')!.value.trim(),
  };
}

function statusText(status: SetupStatus): string {
  switch (status.state) {
    case 'ok': return status.found ? `found ${status.found}` : 'ready';
    case 'missing': return 'missing';
    case 'outdated': return `too old (found ${status.found})`;
    case 'not_logged_in': return status.reason ?? 'not logged in';
    case 'credentials_rejected': return status.reason ?? 'login rejected';
    case 'status_unknown': return status.reason ?? 'status unrecognized';
    case 'status_failed': return status.reason ?? 'status check failed';
    case 'unsupported': return status.reason ?? 'not supported here';
  }
}

/** A failed or unrecognized status check says nothing about the login: the user may already be authenticated. */
function checkUnresolved(status: SetupStatus): boolean {
  return status.state === 'status_unknown' || status.state === 'status_failed';
}

/** Check exactly the choices displayed in the onboarding form. */
async function refreshSetup(): Promise<boolean> {
  const result = await invoke<SetupCheck>('setup_check', { role: nodeRole, choices: setupChoices() });
  const state = document.querySelector<HTMLElement>('#setup-state')!;
  const list = document.querySelector<HTMLElement>('#setup-list')!;
  const button = document.querySelector<HTMLButtonElement>('#setup-standalone')!;
  list.replaceChildren();
  const pending = !result.installed || !result.report?.ready;
  if (!result.installed) {
    state.textContent = 'The GAH CLI is unavailable. Select Install GAH CLI to build it in this window, then retry. Git and Rust must be installed first. Complete bundled release installation is tracked in #1321.';
    button.textContent = 'Install selected configuration';
  } else if (result.error || !result.report) {
    state.textContent = result.error ?? 'Setup could not check this computer.';
    button.textContent = 'Install selected configuration';
  } else {
    const missing = result.report.requirements.filter((item) => item.status.state !== 'ok' && !item.optional).length;
    state.textContent = result.report.ready
      ? 'Everything this computer needs is in place.'
      : `${missing} required ${missing === 1 ? 'item is' : 'items are'} missing. Complete the prerequisites below, then retry installation.`;
    button.textContent = 'Install selected configuration';
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
      if (!ok && checkUnresolved(item.status)) {
        const how = document.createElement('small');
        how.textContent = 'This login may still be valid. Select Check again to re-check it.';
        row.append(how);
      } else if (!ok && item.action) {
        const how = document.createElement('small');
        how.textContent = item.id.includes('login')
          ? 'Sign in using the repository login or coding agent controls below, then check again.'
          : 'Use the official installation guides below, then check again.';
        row.append(how);
      }
      list.append(row);
    }
  }
  list.hidden = list.childElementCount === 0;
  button.hidden = false;
  button.dataset.setupUnavailable = String(!result.installed);
  button.disabled = nativeOnboarding && !result.installed;
  if (!nativeOnboarding) {
    button.dataset.setupUnavailable = 'false';
    button.textContent = 'Open setup in terminal';
    state.textContent = 'Complete setup in the terminal, then select Check again and Save and connect. Windows setup runs in your selected WSL distribution.';
    for (const id of ['setup-install-cli', 'setup-agent-login', 'setup-repository-login']) document.querySelector<HTMLElement>(`#${id}`)!.hidden = true;
  }
  return !pending;
}

async function perform(action: () => Promise<void>) {
  error.textContent = '';
  const buttons = [...document.querySelectorAll('button')].filter(button => !button.closest('#provider-connections, #mistral-section'));
  buttons.forEach((button) => { button.disabled = true; });
  const setupControls = [...document.querySelectorAll<HTMLInputElement | HTMLSelectElement>('#setup-section input, #setup-section select, #repository-provider')];
  setupControls.forEach(control => { control.disabled = true; });
  try { await action(); } catch (err) { error.textContent = String(err); }
  finally {
    buttons.forEach((button) => { button.disabled = button.dataset.setupUnavailable === 'true'; });
    setupControls.forEach(control => { control.disabled = false; });
  }
}

function performSetup(action: () => Promise<void>) {
  return perform(async () => {
    try { await action(); }
    catch (err) {
      document.querySelector('#setup-state')!.textContent = `${String(err)} Correct the failed step and retry using the same button. Your selections are preserved.`;
      throw err;
    }
  });
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
  void performSetup(async () => { await refreshSetup(); });
});
document.querySelector('#setup-standalone')!.addEventListener('click', () => {
  void performSetup(async () => {
    const state = document.querySelector('#setup-state')!;
    if (!nativeOnboarding) {
      central.value = await invoke<string>('open_setup_terminal', { standalone: setupChoices().role === 'standalone' });
      state.textContent = 'Finish setup in the terminal, then select Check again and Save and connect.';
      return;
    }
    state.textContent = 'Checking prerequisites and installing your selected configuration… Native permission dialogs may appear. The first build can take several minutes.';
    const key = document.querySelector<HTMLInputElement>('#setup-gateway-key')!;
    try {
      central.value = await invoke<string>('onboarding_run', { choices: setupChoices(), gatewayKey: key.value });
      state.textContent = 'Setup complete. Opening the local dashboard…';
      await connect();
    } catch (err) {
      state.textContent = `${String(err)} Your choices are preserved; correct the failed step and select Install selected configuration to retry.`;
    } finally { key.value = ''; }
  });
});
document.querySelector('#setup-install-cli')!.addEventListener('click', () => {
  void performSetup(async () => {
    document.querySelector('#setup-state')!.textContent = 'Preparing the GAH CLI… Follow the build progress below. This can take several minutes.';
    await invoke('onboarding_install_cli');
    await refreshSetup();
  });
});
document.querySelector('#setup-agent-login')!.addEventListener('click', () => {
  void performSetup(async () => {
    document.querySelector('#setup-state')!.textContent = 'Signing in… Follow the browser or device-code instructions in the progress panel.';
    await invoke('onboarding_agent_login', { agent: setupChoices().agent });
    document.querySelector('#setup-progress')!.textContent = '';
    await refreshSetup();
  });
});
document.querySelector('#setup-repository-login')!.addEventListener('click', () => {
  void performSetup(async () => {
    const token = document.querySelector<HTMLInputElement>('#setup-repository-token')!;
    try {
      await invoke('onboarding_login', { provider: setupChoices().provider, token: token.value });
      await refreshSetup();
    } finally { token.value = ''; }
  });
});
void listen<string>('gah:onboarding-progress', event => {
  const progress = document.querySelector<HTMLElement>('#setup-progress')!;
  progress.textContent = `${progress.textContent ?? ''}${event.payload}\n`.slice(-24000);
}).catch(() => {});
for (const id of ['setup-mode', 'setup-agent', 'setup-memory', 'repository-provider']) {
  document.querySelector(`#${id}`)!.addEventListener('change', () => { void performSetup(async () => { await refreshSetup(); }); });
}
for (const link of document.querySelectorAll<HTMLAnchorElement>('#setup-section a')) {
  link.addEventListener('click', event => { event.preventDefault(); void invoke('open_external_url', { url: link.href }).catch(err => { error.textContent = `Could not open the guide: ${String(err)}. Open ${link.href} in your browser.`; }); });
}
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
  await loadDesktopUpdate();
});

document.querySelector('#setup-factory')!.addEventListener('change', () => {
  document.querySelector<HTMLElement>('#factory-config')!.hidden = document.querySelector<HTMLSelectElement>('#setup-factory')!.value === 'off';
});
document.querySelector('#setup-factory-configure')!.addEventListener('click', () => {
  void performSetup(async () => {
    await invoke('onboarding_factory', {
      profile: document.querySelector<HTMLInputElement>('#factory-profile')!.value.trim(),
      repo: document.querySelector<HTMLInputElement>('#factory-repo')!.value.trim(),
      localPath: document.querySelector<HTMLInputElement>('#factory-path')!.value.trim(),
      provider: setupChoices().provider,
    });
    document.querySelector('#setup-state')!.textContent = 'Factory profile configured. Review its backend and dispatch settings in the dashboard before starting work.';
  });
});
