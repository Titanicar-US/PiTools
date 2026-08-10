Feature: pull request work-plan controls
  PiTools exposes skip and cancel controls through a GitHub Check Run.

  Scenario: the pull request author cancels a running plan
    Given a running PiTools work plan
    When the pull request author requests cancellation
    Then all remaining work is cancelled

  Scenario: a maintainer skips the current work item
    Given a running PiTools work plan
    When a configured maintainer skips the current item
    Then the next work item is in progress

  Scenario: the pull request author approves a waiting plan
    Given a PiTools work plan waiting for approval
    When the pull request author approves the plan
    Then the approved plan starts its first work item

  Scenario: the pull request author skips a waiting plan item
    Given a PiTools work plan waiting for approval
    When the pull request author skips the planned item
    Then the waiting plan marks the item skipped

  Scenario: the pull request author cancels a waiting plan
    Given a PiTools work plan waiting for approval
    When the pull request author requests cancellation
    Then all remaining work is cancelled

  Scenario: bot branch history rewrites require explicit authorization
    Given a stack branch requires a history rewrite
    Then history rewrites are allowed only for an explicit bot-owned branch

  Scenario: CI evidence is redacted before Pi receives it
    Given a CI failure contains credential-shaped text
    When PiTools prepares CI evidence
    Then the CI evidence contains no credential-shaped text

  Scenario: Actions logs and annotations are retained in bounded diagnosis evidence
    Given a failed GitHub Actions check has logs and annotations
    When PiTools prepares the Actions diagnosis envelope
    Then the bounded evidence retains the Actions log and annotation

  Scenario: Actions logs use the workflow job ID from check-run details
    Given an Actions check run details URL
    When PiTools resolves the workflow job ID for Actions logs
    Then it resolves the workflow job ID without using the check run ID

  Scenario: typed CI patches cannot bypass explicit approval
    Given a typed CI repair patch is proposed without approval
    When PiTools admits the CI mutation
    Then the CI mutation is rejected without explicit approval

  Scenario: non-success Actions conclusions enter CI repair
    Given a cancelled GitHub Actions check
    When PiTools classifies the check conclusion for repair
    Then the CI repair path accepts the check conclusion

  Scenario: repository validation cannot access control-plane credentials
    Given a repository validation sandbox command
    When PiTools builds the validation sandbox
    Then the validation sandbox hides control-plane secrets and network access

  Scenario: Pi worker rejects non-canonical snapshot paths
    Given a Pi worker request with a non-canonical snapshot path
    When PiTools validates the Pi worker request
    Then the Pi worker request is rejected before provider execution

  Scenario: Pi worker rejects an unallowlisted source snapshot
    Given a Pi worker request with an unallowlisted snapshot file
    When PiTools validates the Pi worker request
    Then the Pi worker request is rejected before provider execution

  Scenario: operator metrics expose low-cardinality webhook counters
    Given PiTools has received a webhook
    When the operator reads Prometheus metrics
    Then the received webhook counter is exposed
