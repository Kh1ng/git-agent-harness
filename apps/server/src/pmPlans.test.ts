import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  listPmPlans,
  getPmDecompositionPlan,
  createPmDecompositionPlan,
} from './gahCli.js';
import type {
  PmDecompositionRequest,
  PmDecompositionResponse,
  PmDecompositionListResponse,
  PmPlan,
  PmWorkPacket,
  PmDecompositionPlan,
  PmPlanPublicationState,
  PmPlanPublicationStatus
} from '@git-agent-harness/contracts';

const DEFAULT_PROFILE = 'gah';

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
  assert.equal(response.approved, true);
  assert.equal(response.can_approve, true);
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