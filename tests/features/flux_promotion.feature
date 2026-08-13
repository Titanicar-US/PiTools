Feature: Flux deployment promotion

  Scenario: source Flux is copied into the matching activation layout
    Given a valid PiTools Flux promotion fixture
    When the Flux promotion bundle is synchronized
    Then the target bundle is copied to the matching activation path
    And unrelated target files remain unchanged
