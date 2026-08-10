Feature: pull request work-plan notifications
  PiTools explains planned work in the PR conversation and leaves controls on the Check Run.

  Scenario: starting work announces the plan and its controls
    Given a PiTools work plan is about to start
    When PiTools renders the work-plan comment
    Then the work-plan comment names the planned work and links to its Check Run controls

  Scenario: completed work receives a final summary
    Given a PiTools run has completed with changes and validation
    When PiTools renders the final summary comment
    Then the final summary lists changes, validation, and remaining blockers
