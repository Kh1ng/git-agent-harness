/** Version 1 PM artifact API; Rust owns validation and provider publication. */
export interface PmWorkPacket {
  key: string;
  title: string;
  summary: string;
  objective: string;
  task_class: string;
  difficulty: string;
  risk: string;
  execution_disposition: string;
  recommended_routing: { capability: string; min_tier: string };
  affected_areas: string[];
  affected_files: string[];
  acceptance_criteria: string[];
  verification_commands: string[];
  depends_on: string[];
  duplicate_evidence: string[];
  uncovered_reason: string;
}
export interface PmPlanArtifact {
  schema_version: number;
  profile: string;
  repo: string;
  target: string;
  open_issue_count: number;
  open_mr_count: number;
  merged_mr_count: number;
  ticket_count: number;
  plan: { title: string; summary: string; tickets: PmWorkPacket[] };
}
export interface PmPublicationState {
  schema_version: number;
  plan_fingerprint: string;
  profile: string;
  repo: string;
  source_issue_number: string;
  status: string;
  children: Record<string, {
    key: string; issue_id: string; issue_number: string; url: string;
    parent_linked: boolean; linked_dependencies: string[];
  }>;
}
export interface PmPlanDetail {
  schema_version: 1;
  generated_at: string;
  profile: string;
  id: string;
  provider: string;
  repo: string;
  source_work_id: string;
  updated_at: string;
  artifact: PmPlanArtifact;
  publication: PmPublicationState;
  failures: { timestamp: string; message: string }[];
}
export interface PmPlanList {
  schema_version: 1;
  generated_at: string;
  profile: string;
  plans: {
    id: string; title: string; source_work_id: string; ticket_count: number;
    updated_at: string; publication_status: string; plan_fingerprint: string;
  }[];
  next_cursor: string | null;
  errors: { plan_id: string; message: string }[];
}
export interface PmPlanOperation {
  schema_version: 1;
  generated_at: string;
  dry_run: boolean;
  success: boolean;
  plan: PmPlanDetail;
  output: string[];
  error: string | null;
}
export interface PmPlanPublishRequest {
  profile: string;
  approve: true;
  plan_fingerprint: string;
}
