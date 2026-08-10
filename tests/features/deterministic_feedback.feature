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
