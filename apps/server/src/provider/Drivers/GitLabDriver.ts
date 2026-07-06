/**
 * GitLab Provider Driver
 * Implements the ProviderDriver interface for GitLab
 */

import type { ProviderDriver, ProviderDriverEnv, ProviderDriverInstance } from '../ProviderDriver.js';
import type { ProviderKind, ServerProvider, Session } from '@git-agent-harness/contracts';
import { getProviderRegistry } from '../ProviderRegistry.js';
import { getSessionManager } from '../../sessions/SessionManager.js';

// GitLab-specific environment requirements
export type GitLabDriverEnv = ProviderDriverEnv & {
  GITLAB_PAT?: string;
  GITLAB_API_URL?: string;
  GITLAB_PROJECT_ID?: string;
};

// GitLab provider capabilities
export type GitLabCapabilities = {
  repositories: boolean;
  mergeRequests: boolean;
  issues: boolean;
  pipelines: boolean;
  ciVariables: boolean;
};

class GitLabDriverImpl implements ProviderDriverInstance<GitLabDriverEnv> {
  readonly kind: ProviderKind = 'gitlab';
  readonly version = '1.0.0';
  readonly capabilities: GitLabCapabilities = {
    repositories: true,
    mergeRequests: true,
    issues: true,
    pipelines: true,
    ciVariables: true
  };
  
  constructor(private env: GitLabDriverEnv) {}
  
  async getSnapshot(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    const status = registry.getProviderStatus('gitlab');
    
    return {
      kind: this.kind,
      version: this.version,
      status,
      capabilities: this.capabilities,
      metadata: {
        isAuthenticated: registry.isProviderAuthenticated('gitlab'),
        apiAvailable: this.isGitLabApiAvailable(),
        projectId: this.env.GITLAB_PROJECT_ID,
        apiUrl: this.env.GITLAB_API_URL
      }
    };
  }
  
  async refresh(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    await registry.refreshProviderStatus('gitlab');
    return this.getSnapshot();
  }
  
  async startSession(options: {
    profile: string;
    repo: string;
    branch?: string;
    target?: string;
    mode: string;
    backend?: string;
    model?: string;
    budget?: number;
  }): Promise<Session> {
    const sessionManager = getSessionManager();
    const instanceId = 'gitlab_instance_0';
    
    return sessionManager.startSession({
      providerKind: 'gitlab',
      instanceId,
      ...options
    });
  }
  
  async stopSession(sessionId: string): Promise<Session> {
    const sessionManager = getSessionManager();
    return sessionManager.stopSession(sessionId);
  }
  
  async sendCommand(sessionId: string, command: string): Promise<void> {
    const sessionManager = getSessionManager();
    await sessionManager.sendCommand(sessionId, command);
  }
  
  private isGitLabApiAvailable(): boolean {
    // Check if we have GitLab PAT
    return !!(this.env.GITLAB_PAT || process.env.GITLAB_PAT);
  }
  
  // GitLab-specific methods
  async createMergeRequest(options: {
    projectId: string;
    title: string;
    description: string;
    sourceBranch: string;
    targetBranch: string;
    draft?: boolean;
  }): Promise<{ url: string; iid: number }> {
    console.log(`Creating GitLab MR in project ${options.projectId}: ${options.title}`);
    
    // Simulate MR creation
    return {
      url: `${this.env.GITLAB_API_URL || 'https://gitlab.com'}/${options.projectId}/merge_requests/${Math.floor(Math.random() * 1000)}`,
      iid: Math.floor(Math.random() * 1000)
    };
  }
  
  async getMergeRequestNotes(projectId: string, mrIid: number): Promise<{ id: number; body: string; author: string }[]> {
    // Simulate getting MR notes
    return [
      { id: 1, body: 'Looks good!', author: 'reviewer1' },
      { id: 2, body: 'Can you fix the tests?', author: 'reviewer2' }
    ];
  }
  
  async triggerPipeline(projectId: string, ref: string): Promise<{ id: number; url: string; status: string }> {
    // Simulate triggering pipeline
    return {
      id: Math.floor(Math.random() * 1000),
      url: `${this.env.GITLAB_API_URL || 'https://gitlab.com'}/${projectId}/pipelines/${Math.floor(Math.random() * 1000)}`,
      status: 'pending'
    };
  }
}

// Export the driver and its factory
export const GitLabDriver: ProviderDriver<GitLabDriverEnv> = {
  kind: 'gitlab',
  version: '1.0.0',
  create: (env: GitLabDriverEnv) => new GitLabDriverImpl(env),
  createSnapshot: () => new GitLabDriverImpl({}).getSnapshot()
};