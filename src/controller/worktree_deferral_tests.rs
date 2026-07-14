use super::{resolve_attached_branch_conflicts, NextAction};
use crate::worktree::BranchWorktreeAttachment;
use std::collections::HashSet;
use std::path::PathBuf;

fn fix(work_id: &str, branch: &str) -> NextAction {
    NextAction::FixMr {
        work_id: Some(work_id.to_string()),
        branch: branch.to_string(),
        mr_url: None,
        reason: "test repair".to_string(),
    }
}

#[test]
fn skips_every_attached_repair_before_selecting_runnable_work() {
    let first = fix("#1", "gah/one");
    let mut deferred = Vec::new();
    let mut decisions = 0;

    let replacement = resolve_attached_branch_conflicts(
        &first,
        |branch| {
            Ok(match branch {
                "gah/one" | "gah/two" => Some(BranchWorktreeAttachment {
                    path: PathBuf::from(format!("/tmp/{branch}")),
                    clean: branch == "gah/one",
                }),
                _ => None,
            })
        },
        |branch, work_id, attachment| {
            deferred.push((
                branch.to_string(),
                work_id.map(str::to_string),
                attachment.clean,
            ));
            Ok(())
        },
        |work_ids: &HashSet<String>, branches: &HashSet<String>| {
            decisions += 1;
            match decisions {
                1 => {
                    assert_eq!(work_ids, &HashSet::from(["#1".to_string()]));
                    assert_eq!(branches, &HashSet::from(["gah/one".to_string()]));
                    Ok(fix("#2", "gah/two"))
                }
                2 => {
                    assert_eq!(
                        work_ids,
                        &HashSet::from(["#1".to_string(), "#2".to_string()])
                    );
                    assert_eq!(
                        branches,
                        &HashSet::from(["gah/one".to_string(), "gah/two".to_string()])
                    );
                    Ok(NextAction::DispatchTicket {
                        work_id: Some("#3".to_string()),
                        ticket_path: "#3".to_string(),
                        recommended_backend: None,
                        recommended_model: None,
                        reason: "next runnable item".to_string(),
                    })
                }
                _ => panic!("unexpected extra controller decision"),
            }
        },
    )
    .unwrap()
    .expect("conflicts must produce a replacement action");

    assert_eq!(
        deferred,
        vec![
            ("gah/one".to_string(), Some("#1".to_string()), true),
            ("gah/two".to_string(), Some("#2".to_string()), false),
        ]
    );
    assert!(matches!(
        replacement,
        NextAction::DispatchTicket { ticket_path, .. } if ticket_path == "#3"
    ));
}
