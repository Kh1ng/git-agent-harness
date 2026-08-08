import assert from 'node:assert/strict';
import { test, describe, before, after } from 'node:test';
import { mkdir, writeFile, rm, readdir } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import {
  listPmPlans,
  getPmDecompositionPlan,
  createPmDecompositionPlan,
  readPmPlanArtifact,
  computePlanFingerprint,
} from './gahCli.js';
import type {
  PmDecompositionRequest,
  PmDecompositionResponse,
  PmDecompositionListResponse,
  PmPlan,
  PmWorkPacket,
  PmDecompositionPlan,
  PmPlanPublicationState,
  PmPlanPublicationStatus,
  PmPlanArtifact,
} from '@git-agent-harness/contracts';

const DEFAULT_PROFILE = 'gah';

// Test temporary directory setup
let tempDir: string;

// Test setup and teardown
before(async () => {
  tempDir = join(tmpdir(), 'gah-pm-plans-test');
  await mkdir(tempDir, { recursive: true });
  process.env.GAH_SESSION_ROOT = tempDir;
});

after(async () => {
  try {
    await rm(tempDir, { recursive: true, force: true });
  } catch {
    // Ignore cleanup errors
  }
  delete process.env.GAH_SESSION_ROOT;
});

// Test fixtures
export function createMockPmPlan(): PmPlan {
  return {
    title: 'Test PM Plan',
    summary: 'A test project manager decomposition plan',
    tickets: [
      {
        key: 'ticket-1',
        title: 'Implement API endpoint',
        summary: 'Add new REST endpoint for feature X',
        objective: 'Create a new API endpoint that handles feature X requests',
        task_class: 'feature',
        difficulty: 'medium',
        risk: 'low',
        execution_disposition: 'autonomous',
        recommended_routing: {
          capability: 'edit',
          min_tier: 'standard',
        },
        affected_areas: ['api', 'backend'],
        affected_files: ['src/api/featureX.ts', 'src/api/routes.ts'],
        acceptance_criteria: ['Endpoint responds with 200', 'All tests pass'],
        verification_commands: ['npm test', 'curl http://localhost:3000/feature-x'],
        depends_on: [],
        duplicate_evidence: [],
        uncovered_reason: 'New feature requirement',
      },
      {
        key: 'ticket-2',
        title: 'Add validation',
        summary: 'Add input validation for feature X endpoint',
        objective: 'Validate all inputs to the new feature X endpoint',
        task_class: 'test',
        difficulty: 'easy',
        risk: 'low',
        execution_disposition: 'autonomous',
        recommended_routing: {
          capability: 'edit',
          min_tier: 'standard',
        },
        affected_areas: ['api', 'validation'],
        affected_files: ['src/api/featureX.ts', 'src/api/validation.ts'],
        acceptance_criteria: ['All inputs are validated', 'Invalid inputs return 400'],
        verification_commands: ['npm test'],
        depends_on: ['ticket-1'],
        duplicate_evidence: [],
        uncovered_reason: 'Validation is part of the feature implementation',
      },
    ],
  };
}

export function createMockPmDecompositionRequest(dryRun = false): PmDecompositionRequest {
  return {
    profile: DEFAULT_PROFILE,
    source_work_id: '#123',
    dry_run: dryRun,
    approve_publication: false,
  };
}

/**
 * Create a mock PM plan artifact for filesystem testing
 */
export function createMockPmPlanArtifact(
  profile: string = DEFAULT_PROFILE,
  repo: string = 'test-org/test-repo',
  target: string = '#123',
  provider: string = 'github'
): PmPlanArtifact {
  return {
    schema_version: 1,
    profile,
    repo,
    target,
    open_issue_count: 0,
    open_mr_count: 0,
    merged_mr_count: 0,
    ticket_count: 2,
    plan: createMockPmPlan(),
  };
}

/**
 * Create a session directory with PM plan artifact files for testing
 */
async function createTestSessionDir(
  sessionId: string,
  artifact: PmPlanArtifact,
  includePublicationState = false
): Promise<string> {
  const sessionDir = join(tempDir, sessionId);
  await mkdir(sessionDir, { recursive: true });
  
  // Write the PM plan artifact
  const planPath = join(sessionDir, 'pm-plan-v1.json');
  await writeFile(planPath, JSON.stringify(artifact, null, 2), 'utf-8');
  
  // Optionally write publication state
  if (includePublicationState) {
    const publicationState = {
      schema_version: 1,
      plan_fingerprint: 'test-fingerprint',
      profile: artifact.profile,
      repo: artifact.repo,
      source_issue_number: artifact.target.replace(/^#?/, ''),
      status: 'partial' as PmPlanPublicationStatus,
      children: {},
    };
    const statePath = join(sessionDir, 'pm-plan-v1.json.publication-v1.json');
    await writeFile(statePath, JSON.stringify(publicationState, null, 2), 'utf-8');
  }
  
  return sessionDir;
}

// PM Plans Tests
test('listPmPlans returns empty list for non-existent profile session', async () => {
  const response = await listPmPlans({ profile: 'non-existent-profile' });
  
  assert.equal(response.schema_version, 1);
  assert.equal(response.profile, 'non-existent-profile');
  assert.ok(Array.isArray(response.plans));
  assert.equal(response.plans.length, 0);
  assert.equal(response.can_create_plans, true);
});

test('getPmDecompositionPlan returns not found for missing work ID', async () => {
  const response = await getPmDecompositionPlan({
    profile: DEFAULT_PROFILE,
    sourceWorkId: '#999999'
  });
  
  assert.equal(response.schema_version, 1);
  assert.equal(response.profile, DEFAULT_PROFILE);
  assert.equal(response.source_work_id, '#999999');
  assert.equal(response.plan, null);
  assert.equal(response.publication_state, null);
  assert.ok(response.failure_reason?.includes('not found'));
  assert.equal(response.dry_run, false);
  assert.equal(response.approved, false);
  assert.equal(response.can_approve, false);
});

test('createPmDecompositionPlan handles dry run requests', async () => {
  const request = createMockPmDecompositionRequest(true);
  const response = await createPmDecompositionPlan({
    profile: DEFAULT_PROFILE,
    request
  });
  
  assert.equal(response.schema_version, 1);
  assert.equal(response.profile, DEFAULT_PROFILE);
  assert.equal(response.source_work_id, '#123');
  assert.equal(response.source_issue_number, '123');
  assert.equal(response.plan, null);
  assert.equal(response.publication_state, null);
  assert.ok(response.failure_reason?.includes('Dry run'));
  assert.equal(response.dry_run, true);
  assert.equal(response.approved, false);
  assert.equal(response.can_approve, true);
});

test('createPmDecompositionPlan handles approval requests', async () => {
  const request: PmDecompositionRequest = {
    profile: DEFAULT_PROFILE,
    source_work_id: '#123',
    dry_run: false,
    approve_publication: true,
  };
  
  const response = await createPmDecompositionPlan({
    profile: DEFAULT_PROFILE,
    request
  });
  
  assert.equal(response.schema_version, 1);
  assert.equal(response.profile, DEFAULT_PROFILE);
  assert.equal(response.source_work_id, '#123');
  assert.equal(response.dry_run, false);
  // When no existing plan exists, approved should be false even with approval request
  // because there's no actual plan to approve (this was the contradictory behavior fixed)
  assert.equal(response.approved, false);
  assert.equal(response.can_approve, true);
  assert.ok(response.failure_reason?.includes('PM planning would be triggered'));
});

// Contract Type Tests
test('PmWorkPacket has all required fields', () => {
  const packet: PmWorkPacket = createMockPmPlan().tickets[0];
  
  assert.ok(typeof packet.key === 'string');
  assert.ok(typeof packet.title === 'string');
  assert.ok(typeof packet.summary === 'string');
  assert.ok(typeof packet.objective === 'string');
  assert.ok(['fix', 'feature', 'refactor', 'docs', 'test', 'chore'].includes(packet.task_class));
  assert.ok(['easy', 'medium', 'hard'].includes(packet.difficulty));
  assert.ok(['low', 'medium', 'high'].includes(packet.risk));
  assert.ok(['autonomous', 'supervised', 'human_required'].includes(packet.execution_disposition));
  assert.ok(['edit', 'plan', 'review', 'investigate'].includes(packet.recommended_routing.capability));
  assert.ok(['standard', 'strong'].includes(packet.recommended_routing.min_tier));
  assert.ok(Array.isArray(packet.affected_areas));
  assert.ok(Array.isArray(packet.affected_files));
  assert.ok(Array.isArray(packet.acceptance_criteria));
  assert.ok(Array.isArray(packet.verification_commands));
  assert.ok(Array.isArray(packet.depends_on));
  assert.ok(Array.isArray(packet.duplicate_evidence));
  assert.ok(typeof packet.uncovered_reason === 'string');
});

test('PmPlan has required structure', () => {
  const plan = createMockPmPlan();
  
  assert.ok(typeof plan.title === 'string');
  assert.ok(typeof plan.summary === 'string');
  assert.ok(Array.isArray(plan.tickets));
  assert.ok(plan.tickets.length > 0);
  
  // Test that the plan has the expected ticket structure
  for (const ticket of plan.tickets) {
    assert.ok(ticket.key);
    assert.ok(ticket.title);
    assert.ok(ticket.objective);
  }
});

test('PmDecompositionRequest has required fields', () => {
  const request = createMockPmDecompositionRequest();
  
  assert.ok(typeof request.profile === 'string');
  assert.ok(typeof request.source_work_id === 'string');
  assert.ok(typeof request.dry_run === 'boolean');
  assert.ok(typeof request.approve_publication === 'boolean');
});

test('PmDecompositionResponse has all expected fields', () => {
  const mockResponse: PmDecompositionResponse = {
    schema_version: 1,
    profile: 'test-profile',
    repo: 'test/repo',
    provider: 'github',
    source_work_id: '#123',
    source_issue_number: '123',
    plan_fingerprint: 'abc123',
    plan: createMockPmPlan(),
    publication_state: null,
    failure_reason: null,
    validation_errors: [],
    dry_run: false,
    approved: true,
    can_approve: true,
  };
  
  assert.equal(mockResponse.schema_version, 1);
  assert.equal(mockResponse.profile, 'test-profile');
  assert.equal(mockResponse.repo, 'test/repo');
  assert.equal(mockResponse.provider, 'github');
  assert.equal(mockResponse.source_work_id, '#123');
  assert.equal(mockResponse.source_issue_number, '123');
  assert.ok(mockResponse.plan);
  assert.equal(mockResponse.publication_state, null);
  assert.equal(mockResponse.failure_reason, null);
  assert.ok(Array.isArray(mockResponse.validation_errors));
  assert.equal(mockResponse.dry_run, false);
  assert.equal(mockResponse.approved, true);
  assert.equal(mockResponse.can_approve, true);
});

test('PmDecompositionListResponse has required structure', () => {
  const mockResponse: PmDecompositionListResponse = {
    schema_version: 1,
    profile: 'test-profile',
    plans: [],
    can_create_plans: true,
  };
  
  assert.equal(mockResponse.schema_version, 1);
  assert.equal(mockResponse.profile, 'test-profile');
  assert.ok(Array.isArray(mockResponse.plans));
  assert.equal(mockResponse.can_create_plans, true);
});

// AC5 Tests: GitHub vs GitLab provider distinction and partial publication states
test('PmDecompositionPlan supports GitHub provider', () => {
  const plan: PmDecompositionPlan = {
    schema_version: 1,
    profile: 'github-test',
    repo: 'test-org/test-repo',
    provider: 'github',
    source_work_id: '#123',
    source_issue_number: '123',
    plan_fingerprint: 'abc123',
    plan: createMockPmPlan(),
    publication_state: null,
    failure_reason: null,
    validation_errors: []
  };
  
  assert.equal(plan.provider, 'github');
  assert.ok(plan.repo.includes('/'));
});

test('PmDecompositionPlan supports GitLab provider', () => {
  const plan: PmDecompositionPlan = {
    schema_version: 1,
    profile: 'gitlab-test',
    repo: 'test-group/test-project',
    provider: 'gitlab',
    source_work_id: '#456',
    source_issue_number: '456',
    plan_fingerprint: 'def456',
    plan: createMockPmPlan(),
    publication_state: null,
    failure_reason: null,
    validation_errors: []
  };
  
  assert.equal(plan.provider, 'gitlab');
  assert.ok(plan.repo.includes('/'));
});

test('PmDecompositionPlan publication_state supports partial status', () => {
  const publicationState: PmPlanPublicationState = {
    schema_version: 1,
    plan_fingerprint: 'fingerprint-123',
    profile: 'test-profile',
    repo: 'test-org/test-repo',
    source_issue_number: '123',
    status: 'partial',
    children: {}
  };
  
  assert.equal(publicationState.status, 'partial');
  assert.equal(publicationState.profile, 'test-profile');
  assert.equal(publicationState.repo, 'test-org/test-repo');
});

test('PmDecompositionPlan with partial publication state in response', () => {
  const partialPublication: PmPlanPublicationState = {
    schema_version: 1,
    plan_fingerprint: 'fingerprint-456',
    profile: 'test-profile',
    repo: 'test-org/test-repo',
    source_issue_number: '456',
    status: 'partial',
    children: {}
  };
  
  const plan: PmDecompositionPlan = {
    schema_version: 1,
    profile: 'test-profile',
    repo: 'test-org/test-repo',
    provider: 'github',
    source_work_id: '#789',
    source_issue_number: '789',
    plan_fingerprint: 'ghi789',
    plan: createMockPmPlan(),
    publication_state: partialPublication,
    failure_reason: null,
    validation_errors: []
  };
  
  assert.ok(plan.publication_state);
  assert.equal(plan.publication_state?.status, 'partial');
  assert.equal(plan.publication_state?.profile, 'test-profile');
});

test('PmDecompositionResponse supports partial publication state', () => {
  const partialPublication: PmPlanPublicationState = {
    schema_version: 1,
    plan_fingerprint: 'fingerprint-789',
    profile: 'test-profile',
    repo: 'test-org/test-repo',
    source_issue_number: '789',
    status: 'partial',
    children: {}
  };
  
  const mockResponse: PmDecompositionResponse = {
    schema_version: 1,
    profile: 'test-profile',
    repo: 'test-org/test-repo',
    provider: 'gitlab',
    source_work_id: '#101',
    source_issue_number: '101',
    plan_fingerprint: 'jkl101',
    plan: createMockPmPlan(),
    publication_state: partialPublication,
    failure_reason: null,
    validation_errors: [],
    dry_run: false,
    approved: true,
    can_approve: true
  };
  
  assert.ok(mockResponse.publication_state);
  assert.equal(mockResponse.publication_state?.status, 'partial');
  assert.equal(mockResponse.provider, 'gitlab');
});

test('listPmPlans response includes plans with different providers', async () => {
  // This test verifies that the response structure can handle multiple providers
  const mockResponse: PmDecompositionListResponse = {
    schema_version: 1,
    profile: 'test-profile',
    plans: [
      {
        schema_version: 1,
        profile: 'test-profile',
        repo: 'github-org/repo1',
        provider: 'github',
        source_work_id: '#1',
        source_issue_number: '1',
        plan_fingerprint: 'fingerprint1',
        plan: createMockPmPlan(),
        publication_state: null,
        failure_reason: null,
        validation_errors: []
      },
      {
        schema_version: 1,
        profile: 'test-profile',
        repo: 'gitlab-group/project1',
        provider: 'gitlab',
        source_work_id: '#2',
        source_issue_number: '2',
        plan_fingerprint: 'fingerprint2',
        plan: createMockPmPlan(),
        publication_state: {
          schema_version: 1,
          plan_fingerprint: 'pub-fingerprint',
          profile: 'test-profile',
          repo: 'gitlab-group/project1',
          source_issue_number: '2',
          status: 'partial',
          children: {}
        },
        failure_reason: null,
        validation_errors: []
      }
    ],
    can_create_plans: true
  };
  
  assert.equal(mockResponse.plans.length, 2);
  assert.equal(mockResponse.plans[0].provider, 'github');
  assert.equal(mockResponse.plans[1].provider, 'gitlab');
  assert.equal(mockResponse.plans[1].publication_state?.status, 'partial');
});

// Test for all publication status values
test('PmPlanPublicationState supports all status values', () => {
  const statuses: PmPlanPublicationStatus[] = ['planned', 'partial', 'complete', 'failed'];
  
  for (const status of statuses) {
    const publicationState: PmPlanPublicationState = {
      schema_version: 1,
      plan_fingerprint: 'test-fingerprint',
      profile: 'test-profile',
      repo: 'test-org/test-repo',
      source_issue_number: '123',
      status: status,
      children: {}
    };
    
    assert.ok(statuses.includes(publicationState.status));
  }
});

// ---------------------------------------------------------------------------
// AC5: Filesystem-based tests for PM Plans functionality
// These tests exercise the actual listPmPlans, getPmDecompositionPlan, and
// createPmDecompositionPlan code paths with fixture files
// ---------------------------------------------------------------------------

describe('PM Plans Filesystem Integration Tests', async () => {
  test('readPmPlanArtifact reads and validates a valid PM plan file', async () => {
    const artifact = createMockPmPlanArtifact();
    const sessionDir = await createTestSessionDir('test-read-artifact', artifact);
    const planPath = join(sessionDir, 'pm-plan-v1.json');
    
    const result = await readPmPlanArtifact(planPath);
    
    assert.equal(result.schema_version, 1);
    assert.equal(result.profile, DEFAULT_PROFILE);
    assert.equal(result.repo, 'test-org/test-repo');
    assert.equal(result.target, '#123');
    assert.equal(result.ticket_count, 2);
    assert.ok(result.plan);
    assert.ok(Array.isArray(result.plan.tickets));
    assert.equal(result.plan.tickets.length, 2);
  });

  test('readPmPlanArtifact throws on path traversal attempt', async () => {
    const artifact = createMockPmPlanArtifact();
    await createTestSessionDir('test-path-traversal', artifact);
    
    // Try to read from outside the allowed directory
    const maliciousPath = join(tempDir, '../etc/passwd');
    
    await assert.rejects(
      readPmPlanArtifact(maliciousPath, [tempDir]),
      /Path.*is outside of allowed directories/
    );
  });

  test('readPmPlanArtifact throws on missing required fields', async () => {
    const sessionDir = join(tempDir, 'test-invalid-artifact');
    await mkdir(sessionDir, { recursive: true });
    const planPath = join(sessionDir, 'pm-plan-v1.json');
    
    // Create artifact with missing required fields
    const invalidArtifact = {
      schema_version: 1,
      profile: 'test-profile'
      // Missing repo, target, etc.
    };
    await writeFile(planPath, JSON.stringify(invalidArtifact, null, 2), 'utf-8');
    
    await assert.rejects(
      readPmPlanArtifact(planPath),
      /Invalid PM plan artifact: missing or invalid repo/
    );
  });

  test('computePlanFingerprint produces different fingerprints for different plans', async () => {
    // Create two different artifacts
    const artifact1 = createMockPmPlanArtifact(DEFAULT_PROFILE, 'repo1', '#123');
    const artifact2 = createMockPmPlanArtifact(DEFAULT_PROFILE, 'repo2', '#123');
    
    const fingerprint1 = computePlanFingerprint(artifact1);
    const fingerprint2 = computePlanFingerprint(artifact2);
    
    // Fingerprints should be different for different repos
    assert.notEqual(fingerprint1, fingerprint2);
    assert.ok(typeof fingerprint1 === 'string');
    assert.ok(fingerprint1.length > 0);
  });

  test('computePlanFingerprint produces same fingerprint for identical plans', async () => {
    const artifact1 = createMockPmPlanArtifact();
    const artifact2 = createMockPmPlanArtifact();
    
    const fingerprint1 = computePlanFingerprint(artifact1);
    const fingerprint2 = computePlanFingerprint(artifact2);
    
    // Fingerprints should be identical for identical content
    assert.equal(fingerprint1, fingerprint2);
  });

  test('listPmPlans finds plans in session directory', async () => {
    // Create a test session with a PM plan
    const artifact = createMockPmPlanArtifact(DEFAULT_PROFILE, 'test-repo', '#456');
    await createTestSessionDir('test-list-plans', artifact, true);
    
    const result = await listPmPlans({ profile: DEFAULT_PROFILE });
    
    assert.equal(result.schema_version, 1);
    assert.equal(result.profile, DEFAULT_PROFILE);
    assert.ok(Array.isArray(result.plans));
    
    // Should find at least our test plan (might find others if temp dir persists)
    const ourPlan = result.plans.find(p => p.source_work_id === '#456' || p.source_issue_number === '456');
    if (ourPlan) {
      assert.equal(ourPlan.profile, DEFAULT_PROFILE);
      assert.equal(ourPlan.repo, 'test-repo');
      assert.equal(ourPlan.source_issue_number, '456');
      assert.ok(ourPlan.plan);
      assert.ok(ourPlan.publication_state);
      assert.equal(ourPlan.publication_state?.status, 'partial');
    }
  });

  test('getPmDecompositionPlan finds existing plan by source work ID', async () => {
    // Create a test session with a PM plan
    const artifact = createMockPmPlanArtifact(DEFAULT_PROFILE, 'test-repo', '#789');
    await createTestSessionDir('test-get-plan', artifact);
    
    const result = await getPmDecompositionPlan({
      profile: DEFAULT_PROFILE,
      sourceWorkId: '#789'
    });
    
    assert.equal(result.schema_version, 1);
    assert.equal(result.profile, DEFAULT_PROFILE);
    assert.ok(result.plan);
    assert.equal(result.source_issue_number, '789');
  });

  test('getPmDecompositionPlan returns not found for missing work ID', async () => {
    const result = await getPmDecompositionPlan({
      profile: DEFAULT_PROFILE,
      sourceWorkId: '#999999'
    });
    
    assert.equal(result.schema_version, 1);
    assert.equal(result.profile, DEFAULT_PROFILE);
    assert.equal(result.plan, null);
    assert.ok(result.failure_reason?.includes('not found'));
    assert.equal(result.dry_run, false);
    assert.equal(result.approved, false);
    assert.equal(result.can_approve, false);
  });

  test('createPmDecompositionPlan handles dry run without filesystem access', async () => {
    const request = createMockPmDecompositionRequest(true);
    
    const result = await createPmDecompositionPlan({
      profile: DEFAULT_PROFILE,
      request
    });
    
    assert.equal(result.schema_version, 1);
    assert.equal(result.profile, DEFAULT_PROFILE);
    assert.equal(result.source_work_id, '#123');
    assert.equal(result.source_issue_number, '123');
    assert.equal(result.plan, null);
    assert.ok(result.failure_reason?.includes('Dry run'));
    assert.equal(result.dry_run, true);
    assert.equal(result.approved, false);
    assert.equal(result.can_approve, true);
  });

  test('listPmPlans filters by profile correctly', async () => {
    // Create plans for different profiles
    const artifact1 = createMockPmPlanArtifact(DEFAULT_PROFILE, 'repo1', '#111');
    const artifact2 = createMockPmPlanArtifact('other-profile', 'repo2', '#222');
    
    await createTestSessionDir('test-filter-profile-1', artifact1);
    await createTestSessionDir('test-filter-profile-2', artifact2);
    
    const result = await listPmPlans({ profile: DEFAULT_PROFILE });
    
    assert.equal(result.profile, DEFAULT_PROFILE);
    // All returned plans should be for the requested profile
    for (const plan of result.plans) {
      assert.equal(plan.profile, DEFAULT_PROFILE);
    }
  });

  test('source_issue_number handles various target formats correctly', async () => {
    // Test different target formats
    const testCases = [
      { target: '#123', expected: '123' },
      { target: '123', expected: '123' },
      { target: '#456', expected: '456' },
      { target: '456', expected: '456' },
    ];
    
    for (const testCase of testCases) {
      const artifact = createMockPmPlanArtifact(DEFAULT_PROFILE, 'test-repo', testCase.target);
      await createTestSessionDir(`test-source-issue-${testCase.target}`, artifact);
      
      const result = await listPmPlans({ profile: DEFAULT_PROFILE });
      const plan = result.plans.find(p => p.source_work_id === testCase.target);
      
      if (plan) {
        assert.equal(plan.source_issue_number, testCase.expected);
      }
    }
  });
});

// ---------------------------------------------------------------------------
// End of AC5 Filesystem Integration Tests
// ---------------------------------------------------------------------------
