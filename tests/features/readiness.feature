Feature: advisory pull request readiness
  PiTools reports every blocking gate before a pull request is handed to a human.

  Scenario: merge conflicts and stale branches block readiness
    Given a ready pull request
    And the pull request has a merge conflict and an out-of-date branch
    When PiTools evaluates pull request readiness
    Then the readiness reason includes a merge conflict
    And the readiness reason includes an out-of-date branch

  Scenario: failed required checks block readiness
    Given a ready pull request
    And the pull request has a failed required check
    When PiTools evaluates pull request readiness
    Then the readiness reason includes a failed required check

  Scenario: pending required checks block readiness
    Given a ready pull request
    And the pull request has a pending required check
    When PiTools evaluates pull request readiness
    Then the readiness reason includes a pending required check

  Scenario: missing approval blocks readiness
    Given a ready pull request
    And the pull request has no required approval
    When PiTools evaluates pull request readiness
    Then the readiness reason includes a missing required review

  Scenario: unresolved configured automation feedback blocks readiness
    Given a ready pull request
    And the pull request has unresolved configured automation feedback
    When PiTools evaluates pull request readiness
    Then the readiness reason includes unresolved automation feedback

  Scenario: a fully passing pull request is ready for human merge
    Given a ready pull request
    When PiTools evaluates pull request readiness
    Then the pull request is ready for human merge
