Feature: deterministic automation feedback repair

  Scenario: an approved inline suggestion changes only its exact line range
    Given an approved inline automation suggestion
    When PiTools applies the suggestion
    Then only the suggested lines change
