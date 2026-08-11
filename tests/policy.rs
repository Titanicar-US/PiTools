use pitools::policy::{Policy, PolicyError, ValidationCommand};

#[test]
fn policy_uses_safe_defaults_and_stable_revision() {
    let policy = Policy::from_yaml("{}").expect("empty policy uses defaults");

    assert!(policy.require_approval);
    assert!(!policy.allow_bot_force_push);
    assert_eq!(
        policy.validation_commands,
        vec![ValidationCommand::GitDiffCheck]
    );
    assert_eq!(
        ValidationCommand::GitDiffCheck.argv(),
        &["git", "diff", "--cached", "--check", "--"]
    );
    assert_eq!(policy.revision(), policy.revision());
}

#[test]
fn policy_rejects_fast_reconciliation_or_empty_actors() {
    let error = Policy::from_yaml("reconcile_interval_seconds: 5\nautomation_actors:\n  - ''\n")
        .expect_err("unsafe policy");

    assert!(matches!(error, PolicyError::Invalid(_)));
}

#[test]
fn policy_requires_explicit_repair_path_allowlists() {
    let default = Policy::from_yaml("{}").expect("default policy");
    assert!(default.repair_allowed_paths.is_empty());

    let configured = Policy::from_yaml("repair_allowed_paths:\n  - src/lib.rs\n")
        .expect("configured repair path");
    assert_eq!(configured.repair_allowed_paths, vec!["src/lib.rs"]);
}

#[test]
fn policy_can_declare_explicit_stack_parents() {
    let policy = Policy::from_yaml("stack_parents:\n  101: 100\n").expect("stack parent policy");
    assert_eq!(policy.stack_parents.get(&101), Some(&100));
}

#[test]
fn policy_requires_an_explicit_allowlist_for_bot_branch_rewrites() {
    let policy =
        Policy::from_yaml("allow_bot_force_push: true\nbot_owned_branches:\n  - pitools/stack\n")
            .expect("explicit bot branch policy");
    assert!(policy.allow_bot_force_push);
    assert_eq!(policy.bot_owned_branches, vec!["pitools/stack"]);

    let error = Policy::from_yaml("allow_bot_force_push: true\n")
        .expect_err("force push without a branch allowlist must fail closed");
    assert!(matches!(error, PolicyError::Invalid(_)));
}
