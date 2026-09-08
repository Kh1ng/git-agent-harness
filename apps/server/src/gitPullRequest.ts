import type { ProfileSummary } from '@git-agent-harness/contracts';
import { execProviderCli } from './managerChat/providerCli.js';

/** Creates an MR for the configured GitLab project from the checkout's current
 * branch. An omitted base uses the project's default branch. Provider failures
 * propagate to the route, which must not expose CLI output to the client. */
export async function createGitLabMergeRequest(
  profile: Pick<ProfileSummary, 'repo' | 'web_url' | 'local_path'>,
  input: { title: string; body: string; base?: string; draft: boolean }
): Promise<string> {
  // web_url comes from Rust's configured provider base, never a request host.
  const projectUrl = new URL(profile.web_url ?? '');
  if (projectUrl.protocol !== 'https:' || projectUrl.username || projectUrl.password
    || projectUrl.search || projectUrl.hash || !profile.repo
    || projectUrl.pathname !== `/${profile.repo}`) {
    throw new Error('Invalid GitLab project URL');
  }
  const project = `projects/${encodeURIComponent(profile.repo)}`;
  const { stdout: branch } = await execProviderCli('git', ['branch', '--show-current'], profile.local_path);
  if (!branch.trim()) throw new Error('A source branch is required');
  let target = input.base;
  if (!target) {
    const { stdout } = await execProviderCli('glab', [
      'api', project, '--hostname', projectUrl.host, '--method', 'GET'
    ], profile.local_path);
    const response = JSON.parse(stdout);
    if (typeof response?.default_branch !== 'string' || !response.default_branch.trim()) {
      throw new Error('GitLab project has no default branch');
    }
    target = response.default_branch;
  }
  const title = input.draft && !/^(draft:|\[draft\]|\(draft\))/i.test(input.title)
    ? `Draft: ${input.title}` : input.title;
  const { stdout } = await execProviderCli('glab', [
    'api', `${project}/merge_requests`, '--hostname', projectUrl.host, '--method', 'POST',
    '--raw-field', `source_branch=${branch.trim()}`, '--raw-field', `target_branch=${target}`,
    '--raw-field', `title=${title}`, '--raw-field', `description=${input.body}`
  ], profile.local_path);
  const response = JSON.parse(stdout);
  if (!Number.isSafeInteger(response?.iid) || response.iid <= 0 || typeof response.web_url !== 'string') {
    throw new Error('GitLab returned an invalid merge request');
  }
  const mrUrl = new URL(response.web_url);
  if (mrUrl.origin !== projectUrl.origin || mrUrl.username || mrUrl.password || mrUrl.search || mrUrl.hash
    || mrUrl.pathname !== `${projectUrl.pathname}/-/merge_requests/${response.iid}`) {
    throw new Error('GitLab returned an unexpected merge request URL');
  }
  return mrUrl.href;
}
