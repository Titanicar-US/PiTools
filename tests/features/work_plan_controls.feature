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

  Scenario: operator metrics expose low-cardinality webhook counters
    Given PiTools has received a webhook
    When the operator reads Prometheus metrics
    Then the received webhook counter is exposed
