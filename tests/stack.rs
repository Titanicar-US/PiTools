#[path = "../src/stack.rs"]
mod stack;

use std::collections::BTreeSet;

use stack::{
    BranchState, PushMode, RebaseOutcome, RebasePlanner, RebasePolicy, RebaseTarget, StackError,
    StackPlanner, StackPullRequest,
};

fn pull_request(number: i32, head_branch: &str, base_branch: &str) -> StackPullRequest {
    StackPullRequest {
        number,
        head_branch: head_branch.into(),
        base_branch: base_branch.into(),
        explicit_parent: None,
    }
}

#[test]
fn stack_plan_orders_pull_requests_bottom_to_top() {
    let bottom = pull_request(10, "stack/bottom", "main");
    let middle = pull_request(20, "stack/middle", "stack/bottom");
    let mut top = pull_request(30, "stack/top", "stack/middle");
    top.explicit_parent = Some(20);

    let plan = StackPlanner::plan(&[top, bottom, middle]).expect("valid stack");

    assert_eq!(plan.merge_order, vec![10, 20, 30]);
    assert!(plan.required_base_updates.is_empty());
}

#[test]
fn stack_partition_keeps_unrelated_pull_requests_out_of_the_stack() {
    let bottom = pull_request(10, "stack/bottom", "main");
    let top = pull_request(20, "stack/top", "stack/bottom");
    let unrelated = pull_request(30, "feature/unrelated", "main");

    let partitions = StackPlanner::partition(&[top, unrelated, bottom]);

    assert_eq!(partitions.len(), 2);
    assert_eq!(
        partitions
            .iter()
            .map(|items| items.iter().map(|item| item.number).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        vec![vec![10, 20], vec![30]]
    );
}

#[test]
fn explicit_parent_plans_the_required_base_update() {
    let bottom = pull_request(10, "stack/bottom", "main");
    let mut top = pull_request(20, "stack/top", "main");
    top.explicit_parent = Some(10);

    let plan = StackPlanner::plan(&[top, bottom]).expect("explicit stack");

    assert_eq!(plan.merge_order, vec![10, 20]);
    assert_eq!(plan.required_base_updates.len(), 1);
    assert_eq!(plan.required_base_updates[0].pull_request, 20);
    assert_eq!(plan.required_base_updates[0].from_branch, "main");
    assert_eq!(plan.required_base_updates[0].to_branch, "stack/bottom");
}

#[test]
fn stack_plan_rejects_cycles() {
    let mut first = pull_request(10, "stack/first", "stack/second");
    first.explicit_parent = Some(20);
    let mut second = pull_request(20, "stack/second", "stack/first");
    second.explicit_parent = Some(10);

    let error = StackPlanner::plan(&[first, second]).expect_err("cycle must be rejected");

    assert_eq!(error, StackError::CycleDetected(vec![10, 20]));
}

#[test]
fn stack_plan_rejects_ambiguous_order() {
    let first = pull_request(10, "stack/first", "main");
    let second = pull_request(20, "stack/second", "main");

    let error = StackPlanner::plan(&[second, first]).expect_err("two roots are ambiguous");

    assert_eq!(error, StackError::AmbiguousOrder(vec![10, 20]));
}

#[test]
fn rebase_plan_rejects_a_stale_branch() {
    let branch = BranchState {
        name: "stack/top".into(),
        clean: true,
        current: false,
        bot_owned: true,
    };

    let error = RebasePlanner::plan(&branch, &RebaseTarget::rebase_onto("stack/bottom"))
        .expect_err("stale branch must be refreshed first");

    assert_eq!(error, StackError::StaleBranch("stack/top".into()));
}

#[test]
fn rebase_plan_rejects_a_dirty_branch() {
    let branch = BranchState {
        name: "stack/top".into(),
        clean: false,
        current: true,
        bot_owned: true,
    };

    let error = RebasePlanner::plan(&branch, &RebaseTarget::rebase_onto("stack/bottom"))
        .expect_err("dirty branch must be cleaned first");

    assert_eq!(error, StackError::DirtyBranch("stack/top".into()));
}

#[test]
fn rebase_plan_allows_a_clean_current_branch() {
    let branch = BranchState {
        name: "stack/top".into(),
        clean: true,
        current: true,
        bot_owned: true,
    };

    let plan = RebasePlanner::plan(&branch, &RebaseTarget::rebase_onto("stack/bottom"))
        .expect("clean current branch can be planned");

    assert_eq!(plan.outcome, RebaseOutcome::Rebase);
    assert_eq!(plan.push_mode, PushMode::ForceWithLease);
}

#[test]
fn rebase_plan_prefers_fast_forward_without_force() {
    let branch = BranchState {
        name: "stack/top".into(),
        clean: true,
        current: true,
        bot_owned: false,
    };

    let plan = RebasePlanner::plan(&branch, &RebaseTarget::fast_forward_to("stack/bottom"))
        .expect("fast-forward plan");

    assert_eq!(plan.outcome, RebaseOutcome::FastForward);
    assert_eq!(plan.push_mode, PushMode::FastForwardOnly);
    assert_eq!(
        RebasePolicy::default().authorize_push(&branch, &plan),
        Ok(())
    );
}

#[test]
fn rebase_plan_surfaces_conflicts_without_a_push() {
    let branch = BranchState {
        name: "stack/top".into(),
        clean: true,
        current: true,
        bot_owned: true,
    };

    let plan = RebasePlanner::plan(&branch, &RebaseTarget::conflicting_with("stack/bottom"))
        .expect("conflict is a non-mutating plan outcome");

    assert_eq!(plan.outcome, RebaseOutcome::Conflict);
    assert_eq!(plan.push_mode, PushMode::NoPush);
}

#[test]
fn default_policy_refuses_force_push() {
    let branch = BranchState {
        name: "stack/top".into(),
        clean: true,
        current: true,
        bot_owned: true,
    };
    let plan = RebasePlanner::plan(&branch, &RebaseTarget::rebase_onto("stack/bottom"))
        .expect("rebase plan");

    let error = RebasePolicy::default()
        .authorize_push(&branch, &plan)
        .expect_err("force push is disabled by default");

    assert_eq!(error, StackError::ForcePushProhibited("stack/top".into()));
}

#[test]
fn rewrite_requires_an_explicit_bot_owned_branch_policy() {
    let policy = RebasePolicy {
        allow_bot_owned_rewrite: true,
        approved_bot_owned_branches: BTreeSet::from(["pitools/topic".into()]),
    };
    let human_branch = BranchState {
        name: "users/alice/topic".into(),
        clean: true,
        current: true,
        bot_owned: false,
    };
    let plan = RebasePlanner::plan(&human_branch, &RebaseTarget::rebase_onto("stack/bottom"))
        .expect("rebase plan");

    assert_eq!(
        policy.authorize_push(&human_branch, &plan),
        Err(StackError::RewriteRequiresBotOwnedBranch(
            "users/alice/topic".into()
        ))
    );

    let bot_branch = BranchState {
        name: "pitools/topic".into(),
        bot_owned: true,
        ..human_branch
    };
    let bot_plan = RebasePlanner::plan(&bot_branch, &RebaseTarget::rebase_onto("stack/bottom"))
        .expect("rebase plan");

    assert_eq!(policy.authorize_push(&bot_branch, &bot_plan), Ok(()));
}
