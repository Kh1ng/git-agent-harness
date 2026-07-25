//! Controller facade (TICKET-077 through TICKET-083, #357): durable action
//! schema (`action`), pure decision policy (`decision`), abandoned-run and
//! stuck-loop recovery (`recovery`), and daemon/dispatch runtime
//! orchestration (`runtime`) each live in their own bounded module. This
//! file only re-exports the caller-facing surface.

mod action;
pub use self::action::NextAction;

mod decision;
pub use self::decision::decide_next_action;
pub(crate) use self::decision::{is_genuine_agent_failure, AUTO_RETRY_CAP};

mod human_required_reason;
pub use self::human_required_reason::HumanRequiredReason;

mod remediation;
pub use self::remediation::{
    plan_remediation, RemediationAction, RemediationActionKind, RemediationAuthority,
    RemediationContext, RemediationPlan,
};

mod ownership;
mod recovery;

mod runtime;
pub(crate) use self::runtime::execute_action;
#[allow(unused_imports)]
pub(crate) use self::runtime::loop_parallel_argument;
#[cfg(test)]
pub(crate) use self::runtime::test_node_lease;
pub use self::runtime::RouteNodeAdmission;
pub(crate) use self::runtime::{NodeAdmissionDeferred, WorkerNodeLease};
// The CLI facade is the only production caller of this crate-private path.
// Keep the explicit allowance because library reachability analysis otherwise
// reports the re-export as unused in non-binary targets.
#[allow(unused_imports)]
pub(crate) use self::runtime::run_dispatch_and_record;
pub use self::runtime::{acquire_profile_lock, run_loop, run_once};
