import { repositoryCli } from '@git-agent-harness/contracts';

type RepositoryTool = { program: string; installed: boolean };
type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

/** Installation and login are separate prerequisites. This check works even
 * before gah is installed, so the first-run app can explain what is missing. */
export function bindRepositoryTools(section: HTMLElement, invoke: Invoke) {
  const provider = section.querySelector<HTMLSelectElement>('#repository-provider')!;
  const status = section.querySelector<HTMLElement>('#repository-tool-status')!;
  const guide = section.querySelector<HTMLAnchorElement>('#repository-tool-guide')!;
  const check = section.querySelector<HTMLButtonElement>('#repository-tool-check')!;
  let revision = 0;
  async function refresh() {
    const request = ++revision;
    const program = provider.value;
    const tool = repositoryCli(program)!;
    status.textContent = `Checking ${tool.label} (${program})…`;
    guide.href = tool.installUrl;
    guide.textContent = `Install ${tool.label}`;
    guide.hidden = false;
    try {
      const tools = await invoke<RepositoryTool[]>('repository_tools');
      if (request !== revision) return;
      const installed = tools.find(entry => entry.program === program)?.installed;
      status.textContent = installed
        ? `${tool.label} (${program}) is installed. Sign in next from the dashboard. Installation alone does not verify your login.`
        : `${tool.label} (${program}) is required to read issues and open pull requests for ${program === 'gh' ? 'GitHub' : 'GitLab'} repositories. Install it before signing in, then select Check installation.`;
      guide.hidden = !!installed;
    } catch {
      if (request === revision) status.textContent = 'Installation could not be checked. Use the official guide, then retry Check installation.';
    }
  }
  provider.addEventListener('change', () => { void refresh(); });
  check.addEventListener('click', () => { void refresh(); });
  guide.addEventListener('click', event => {
    event.preventDefault();
    void invoke('open_external_url', { url: guide.href }).catch(() => {
      status.textContent = `The guide could not open. Open ${guide.href} in your browser.`;
    });
  });
  void refresh();
}
