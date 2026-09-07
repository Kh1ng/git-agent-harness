import { invoke } from '@tauri-apps/api/core';

type Settings = { central_url: string; wsl_distribution: string };
type WorkerStatus = { running: boolean; note: string; tools: { name: string; environment: string; installed: boolean }[] };
const central = document.querySelector<HTMLInputElement>('#central-url')!;
const distribution = document.querySelector<HTMLInputElement>('#wsl-distribution')!;
const error = document.querySelector<HTMLElement>('#error')!;
const status = document.querySelector<HTMLElement>('#status')!;

async function perform(action: () => Promise<void>) {
  error.textContent = '';
  document.querySelectorAll('button').forEach((button) => { button.disabled = true; });
  try { await action(); } catch (err) { error.textContent = String(err); }
  finally { document.querySelectorAll('button').forEach((button) => { button.disabled = false; }); }
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

document.querySelector('#connection')!.addEventListener('submit', (event) => {
  event.preventDefault();
  void perform(async () => {
    await invoke('connect_dashboard', { settings: { central_url: central.value.trim(), wsl_distribution: distribution.value.trim() } });
    status.textContent = 'Dashboard window opened. If it cannot connect, check the address and network here, then try again.';
  });
});
document.querySelector('#refresh')!.addEventListener('click', () => { void perform(refresh); });
for (const [id, running] of [['start', true], ['stop', false]] as const) {
  document.querySelector(`#${id}`)!.addEventListener('click', () => {
    void perform(async () => { await invoke('set_worker_running', { running }); await refresh(); });
  });
}
void perform(async () => {
  const settings = await invoke<Settings>('desktop_settings');
  central.value = settings.central_url;
  distribution.value = settings.wsl_distribution;
  if (navigator.userAgent.includes('Windows')) {
    distribution.hidden = false;
    document.querySelector<HTMLElement>('#wsl-label')!.hidden = false;
  }
});
