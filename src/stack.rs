use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Pull request fields used to derive one linear stack.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackPullRequest {
    pub number: i32,
    pub head_branch: String,
    pub base_branch: String,
    pub explicit_parent: Option<i32>,
}

/// A base branch correction required to match an explicit parent declaration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BaseUpdate {
    pub pull_request: i32,
    pub from_branch: String,
    pub to_branch: String,
}

/// A deterministic, base-most to tip-most stack plan.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackPlan {
    pub merge_order: Vec<i32>,
    pub required_base_updates: Vec<BaseUpdate>,
}

/// Pure stack graph planner. It performs no GitHub or git operations.
pub struct StackPlanner;

impl StackPlanner {
    /// Partition observed pull requests into independent stack components.
    ///
    /// A shared base branch alone is not a stack relationship: only an inferred
    /// head-to-base edge or an explicit parent declaration connects components.
    pub fn partition(pull_requests: &[StackPullRequest]) -> Vec<Vec<StackPullRequest>> {
        let mut by_number = BTreeMap::new();
        let mut by_head: BTreeMap<&str, Vec<i32>> = BTreeMap::new();
        for pull_request in pull_requests {
            by_number.insert(pull_request.number, pull_request);
            by_head
                .entry(&pull_request.head_branch)
                .or_default()
                .push(pull_request.number);
        }

        let mut adjacency: BTreeMap<i32, BTreeSet<i32>> = by_number
            .keys()
            .map(|number| (*number, BTreeSet::new()))
            .collect();
        for pull_request in pull_requests {
            let inferred = by_head
                .get(pull_request.base_branch.as_str())
                .and_then(|parents| (parents.len() == 1).then_some(parents[0]));
            if let Some(parent) = pull_request.explicit_parent.or(inferred)
                && by_number.contains_key(&parent)
            {
                adjacency
                    .entry(pull_request.number)
                    .or_default()
                    .insert(parent);
                adjacency
                    .entry(parent)
                    .or_default()
                    .insert(pull_request.number);
            }
        }

        let mut components = Vec::new();
        let mut visited = BTreeSet::new();
        for &number in by_number.keys() {
            if !visited.insert(number) {
                continue;
            }
            let mut queue = vec![number];
            let mut component_numbers = Vec::new();
            while let Some(current) = queue.pop() {
                component_numbers.push(current);
                for &neighbor in adjacency.get(&current).into_iter().flatten() {
                    if visited.insert(neighbor) {
                        queue.push(neighbor);
                    }
                }
            }
            component_numbers.sort_unstable();
            components.push(
                component_numbers
                    .into_iter()
                    .filter_map(|number| by_number.get(&number).map(|item| (*item).clone()))
                    .collect(),
            );
        }
        components
    }

    pub fn plan(pull_requests: &[StackPullRequest]) -> Result<StackPlan, StackError> {
        if pull_requests.is_empty() {
            return Err(StackError::EmptyStack);
        }

        let mut by_number = BTreeMap::new();
        let mut by_head: BTreeMap<&str, Vec<i32>> = BTreeMap::new();
        for pull_request in pull_requests {
            if by_number
                .insert(pull_request.number, pull_request)
                .is_some()
            {
                return Err(StackError::DuplicatePullRequest(pull_request.number));
            }
            by_head
                .entry(&pull_request.head_branch)
                .or_default()
                .push(pull_request.number);
        }

        let mut parents = BTreeMap::new();
        let mut required_base_updates = Vec::new();
        for (&number, pull_request) in &by_number {
            let inferred = by_head
                .get(pull_request.base_branch.as_str())
                .cloned()
                .unwrap_or_default();
            if inferred.len() > 1 {
                return Err(StackError::AmbiguousParents {
                    pull_request: number,
                    candidates: inferred,
                });
            }

            let parent = match pull_request.explicit_parent {
                Some(parent) => {
                    let parent_pull_request =
                        by_number.get(&parent).ok_or(StackError::MissingParent {
                            pull_request: number,
                            parent,
                        })?;
                    if let Some(&inferred_parent) = inferred.first()
                        && inferred_parent != parent
                    {
                        return Err(StackError::ConflictingParent {
                            pull_request: number,
                            explicit_parent: parent,
                            inferred_parent,
                        });
                    }
                    if pull_request.base_branch != parent_pull_request.head_branch {
                        required_base_updates.push(BaseUpdate {
                            pull_request: number,
                            from_branch: pull_request.base_branch.clone(),
                            to_branch: parent_pull_request.head_branch.clone(),
                        });
                    }
                    Some(parent)
                }
                None => inferred.first().copied(),
            };
            parents.insert(number, parent);
        }

        reject_cycles(&parents)?;

        let roots: Vec<i32> = parents
            .iter()
            .filter_map(|(&number, parent)| parent.is_none().then_some(number))
            .collect();
        if roots.len() != 1 {
            return Err(StackError::AmbiguousOrder(roots));
        }

        let mut children: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
        for (&child, parent) in &parents {
            if let Some(parent) = parent {
                children.entry(*parent).or_default().push(child);
            }
        }
        if let Some(siblings) = children.values().find(|siblings| siblings.len() > 1) {
            return Err(StackError::AmbiguousOrder(siblings.clone()));
        }

        let mut merge_order = Vec::with_capacity(pull_requests.len());
        let mut current = roots[0];
        loop {
            merge_order.push(current);
            match children.get(&current).and_then(|items| items.first()) {
                Some(&child) => current = child,
                None => break,
            }
        }

        if merge_order.len() != pull_requests.len() {
            let ordered: BTreeSet<_> = merge_order.iter().copied().collect();
            let omitted = by_number
                .keys()
                .filter(|number| !ordered.contains(number))
                .copied()
                .collect();
            return Err(StackError::AmbiguousOrder(omitted));
        }

        Ok(StackPlan {
            merge_order,
            required_base_updates,
        })
    }
}

fn reject_cycles(parents: &BTreeMap<i32, Option<i32>>) -> Result<(), StackError> {
    for &start in parents.keys() {
        let mut path = Vec::new();
        let mut positions = BTreeMap::new();
        let mut current = start;

        loop {
            if let Some(&position) = positions.get(&current) {
                let mut cycle = path[position..].to_vec();
                cycle.sort_unstable();
                return Err(StackError::CycleDetected(cycle));
            }
            positions.insert(current, path.len());
            path.push(current);
            match parents.get(&current).copied().flatten() {
                Some(parent) => current = parent,
                None => break,
            }
        }
    }
    Ok(())
}

/// Local and observed-remote state required before planning a branch update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchState {
    pub name: String,
    pub clean: bool,
    pub current: bool,
    pub bot_owned: bool,
}

/// The observed relationship between a branch and its intended target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebaseTarget {
    pub branch: String,
    pub can_fast_forward: bool,
    pub has_conflicts: bool,
}

impl RebaseTarget {
    pub fn rebase_onto(branch: impl Into<String>) -> Self {
        Self {
            branch: branch.into(),
            can_fast_forward: false,
            has_conflicts: false,
        }
    }

    pub fn fast_forward_to(branch: impl Into<String>) -> Self {
        Self {
            branch: branch.into(),
            can_fast_forward: true,
            has_conflicts: false,
        }
    }

    pub fn conflicting_with(branch: impl Into<String>) -> Self {
        Self {
            branch: branch.into(),
            can_fast_forward: false,
            has_conflicts: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebaseOutcome {
    FastForward,
    Rebase,
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushMode {
    NoPush,
    FastForwardOnly,
    ForceWithLease,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebasePlan {
    pub branch: String,
    pub target_branch: String,
    pub outcome: RebaseOutcome,
    pub push_mode: PushMode,
}

/// Pure rebase planner. A rewrite plan still requires separate push authorization.
pub struct RebasePlanner;

impl RebasePlanner {
    pub fn plan(branch: &BranchState, target: &RebaseTarget) -> Result<RebasePlan, StackError> {
        if !branch.clean {
            return Err(StackError::DirtyBranch(branch.name.clone()));
        }
        if !branch.current {
            return Err(StackError::StaleBranch(branch.name.clone()));
        }

        let (outcome, push_mode) = if target.has_conflicts {
            (RebaseOutcome::Conflict, PushMode::NoPush)
        } else if target.can_fast_forward {
            (RebaseOutcome::FastForward, PushMode::FastForwardOnly)
        } else {
            (RebaseOutcome::Rebase, PushMode::ForceWithLease)
        };
        Ok(RebasePlan {
            branch: branch.name.clone(),
            target_branch: target.branch.clone(),
            outcome,
            push_mode,
        })
    }
}

/// Service policy for authorizing a planned branch push.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RebasePolicy {
    pub allow_bot_owned_rewrite: bool,
    pub approved_bot_owned_branches: BTreeSet<String>,
}

impl RebasePolicy {
    pub fn authorize_push(
        &self,
        branch: &BranchState,
        plan: &RebasePlan,
    ) -> Result<(), StackError> {
        if plan.branch != branch.name {
            return Err(StackError::PlanBranchMismatch {
                planned: plan.branch.clone(),
                actual: branch.name.clone(),
            });
        }
        if plan.push_mode != PushMode::ForceWithLease {
            return Ok(());
        }
        if !self.allow_bot_owned_rewrite {
            return Err(StackError::ForcePushProhibited(branch.name.clone()));
        }
        if !branch.bot_owned || !self.approved_bot_owned_branches.contains(&branch.name) {
            return Err(StackError::RewriteRequiresBotOwnedBranch(
                branch.name.clone(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StackError {
    #[error("a stack must contain at least one pull request")]
    EmptyStack,
    #[error("pull request {0} appears more than once")]
    DuplicatePullRequest(i32),
    #[error("pull request {pull_request} declares missing parent {parent}")]
    MissingParent { pull_request: i32, parent: i32 },
    #[error("pull request {pull_request} has ambiguous parents {candidates:?}")]
    AmbiguousParents {
        pull_request: i32,
        candidates: Vec<i32>,
    },
    #[error(
        "pull request {pull_request} declares parent {explicit_parent} but its base implies {inferred_parent}"
    )]
    ConflictingParent {
        pull_request: i32,
        explicit_parent: i32,
        inferred_parent: i32,
    },
    #[error("stack contains a cycle through pull requests {0:?}")]
    CycleDetected(Vec<i32>),
    #[error("stack order is ambiguous across pull requests {0:?}")]
    AmbiguousOrder(Vec<i32>),
    #[error("branch {0} has uncommitted changes")]
    DirtyBranch(String),
    #[error("branch {0} is not current with its observed remote head")]
    StaleBranch(String),
    #[error("force-push is prohibited for branch {0}")]
    ForcePushProhibited(String),
    #[error("history rewrite requires a bot-owned branch: {0}")]
    RewriteRequiresBotOwnedBranch(String),
    #[error("rebase plan branch {planned} does not match branch {actual}")]
    PlanBranchMismatch { planned: String, actual: String },
}
