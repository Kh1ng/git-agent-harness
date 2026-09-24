import type { GitReviewPullRequest, ProfileSummary } from '@git-agent-harness/contracts';
import { execProviderCli } from './managerChat/providerCli.js';

type PullRequestProfile = Pick<ProfileSummary, 'provider' | 'repo' | 'web_url' | 'local_path'>;

function validatedProjectUrl(profile: PullRequestProfile): URL {
  const url = new URL(profile.web_url ?? '');
  if (url.protocol !== 'https:' || url.username || url.password || url.search || url.hash
    || !profile.repo || url.pathname !== `/${profile.repo}`) throw new Error('Invalid project URL');
  return url;
}

function providerRequest(profile: PullRequestProfile, value: unknown): GitReviewPullRequest {
  if (!value || typeof value !== 'object') throw new Error('Provider returned an invalid pull request');
  const request = value as Record<string, unknown>;
  const number = request.number ?? request.iid;
  const rawUrl = request.url ?? request.web_url;
  const title = request.title;
  if (!Number.isSafeInteger(number) || (number as number) <= 0 || typeof rawUrl !== 'string' || typeof title !== 'string') {
    throw new Error('Provider returned an invalid pull request');
  }
  const url = new URL(rawUrl);
  const project = validatedProjectUrl(profile);
  const suffix = profile.provider === 'gitlab' ? `/-/merge_requests/${number}` : `/pull/${number}`;
  if (url.origin !== project.origin || url.username || url.password || url.search || url.hash
    || url.pathname !== `${project.pathname}${suffix}`) throw new Error('Provider returned an unexpected pull request URL');
  const draft = request.isDraft === true || request.draft === true || request.work_in_progress === true
    || /^(draft:|\[draft\]|\(draft\))/i.test(title);
  return { number: number as number, title, url: url.href, draft };
}

/** Finds the one open PR/MR for a branch. Provider output is validated before
 * it reaches the browser, and an empty result is the ordinary no-PR state. */
export async function findOpenPullRequest(profile: PullRequestProfile, cwd: string, branch: string): Promise<GitReviewPullRequest | null> {
  if (profile.provider === 'github') {
    const { stdout } = await execProviderCli('gh', ['pr', 'list', '--head', branch, '--state', 'open', '--limit', '1', '--json', 'number,title,url,isDraft'], cwd);
    const rows = JSON.parse(stdout);
    if (!Array.isArray(rows)) throw new Error('GitHub returned an invalid pull request list');
    return rows[0] === undefined ? null : providerRequest(profile, rows[0]);
  }
  if (profile.provider !== 'gitlab') throw new Error('Unsupported repository provider');
  const url = validatedProjectUrl(profile);
  const project = `projects/${encodeURIComponent(profile.repo)}`;
  const { stdout } = await execProviderCli('glab', [
    'api', `${project}/merge_requests`, '--hostname', url.host, '--method', 'GET',
    '--raw-field', 'state=opened', '--raw-field', `source_branch=${branch}`
  ], cwd);
  const rows = JSON.parse(stdout);
  if (!Array.isArray(rows)) throw new Error('GitLab returned an invalid merge request list');
  return rows[0] === undefined ? null : providerRequest(profile, rows[0]);
}

/** Creates an MR for the configured GitLab project from the checkout's current
 * branch. An omitted base uses the project's default branch. Provider failures
 * propagate to the route, which must not expose CLI output to the client. */
export async function createGitLabMergeRequest(
  profile: PullRequestProfile,
  input: { title: string; body: string; base?: string; draft: boolean },
  cwd = profile.local_path
): Promise<string> {
  // web_url comes from Rust's configured provider base, never a request host.
  const projectUrl = validatedProjectUrl(profile);
  const project = `projects/${encodeURIComponent(profile.repo)}`;
  const { stdout: branch } = await execProviderCli('git', ['branch', '--show-current'], cwd);
  if (!branch.trim()) throw new Error('A source branch is required');
  let target = input.base;
  if (!target) {
    const { stdout } = await execProviderCli('glab', [
      'api', project, '--hostname', projectUrl.host, '--method', 'GET'
    ], cwd);
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
  ], cwd);
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

/** Pushes the reviewed HEAD, then creates or updates its provider request.
 * Nothing in this function reads the dirty worktree for PR text or content. */
export async function publishPullRequest(
  profile: PullRequestProfile,
  cwd: string,
  input: { title: string; body: string; base: string; draft: boolean }
): Promise<{ url: string; existing: boolean }> {
  await execProviderCli('git', ['push', '--set-upstream', 'origin', 'HEAD'], cwd);
  const { stdout: branchOutput } = await execProviderCli('git', ['branch', '--show-current'], cwd);
  const branch = branchOutput.trim();
  if (!branch) throw new Error('A source branch is required');
  const existing = await findOpenPullRequest(profile, cwd, branch);
  if (profile.provider === 'github') {
    if (!existing) {
      const args = ['pr', 'create', '--title', input.title, '--body', input.body, '--base', input.base, '--head', branch];
      if (input.draft) args.push('--draft');
      const { stdout } = await execProviderCli('gh', args, cwd);
      return { url: providerRequest(profile, { number: Number(new URL(stdout.trim()).pathname.split('/').at(-1)), title: input.title, url: stdout.trim(), isDraft: input.draft }).url, existing: false };
    }
    await execProviderCli('gh', ['pr', 'edit', String(existing.number), '--title', input.title, '--body', input.body, '--base', input.base], cwd);
    if (input.draft !== existing.draft) {
      await execProviderCli('gh', input.draft
        ? ['pr', 'ready', '--undo', String(existing.number)]
        : ['pr', 'ready', String(existing.number)], cwd);
    }
    return { url: existing.url, existing: true };
  }
  if (profile.provider !== 'gitlab') throw new Error('Unsupported repository provider');
  if (!existing) return { url: await createGitLabMergeRequest(profile, input, cwd), existing: false };
  const url = validatedProjectUrl(profile);
  const title = input.draft && !/^(draft:|\[draft\]|\(draft\))/i.test(input.title) ? `Draft: ${input.title}` : input.title;
  const { stdout } = await execProviderCli('glab', [
    'api', `projects/${encodeURIComponent(profile.repo)}/merge_requests/${existing.number}`,
    '--hostname', url.host, '--method', 'PUT',
    '--raw-field', `target_branch=${input.base}`, '--raw-field', `title=${title}`, '--raw-field', `description=${input.body}`
  ], cwd);
  return { url: providerRequest(profile, JSON.parse(stdout)).url, existing: true };
}
