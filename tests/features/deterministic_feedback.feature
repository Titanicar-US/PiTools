Feature: deterministic automation feedback repair

  Scenario: an approved inline suggestion changes only its exact line range
    Given an approved inline automation suggestion
    When PiTools applies the suggestion
    Then only the suggested lines change

  Scenario: applied automation feedback receives an outcome comment before resolution
    Given an applied automation feedback outcome
    When PiTools renders the feedback outcome comment
    Then the outcome comment says the suggestion was applied and the thread is being resolved

  Scenario: rejected automation feedback receives an outcome comment before resolution
    Given a rejected automation feedback outcome
    When PiTools renders the feedback outcome comment
    Then the outcome comment says the suggestion was rejected and human follow-up is required

  Scenario: applied issue-comment automation feedback receives a PR-level outcome comment
    Given an applied issue-comment automation feedback outcome
    When PiTools renders the feedback outcome comment
    Then the issue-comment outcome explains that no review thread is available

  Scenario: rejected issue-comment automation feedback receives a PR-level outcome comment
    Given a rejected issue-comment automation feedback outcome
    When PiTools renders the feedback outcome comment
    Then the issue-comment outcome says the suggestion was rejected without claiming thread resolution
