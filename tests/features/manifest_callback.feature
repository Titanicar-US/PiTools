Feature: GitHub App manifest setup

  Scenario: unsafe manifest conversion codes fail closed
    Given an unsafe GitHub App manifest conversion code
    When PiTools validates the manifest conversion code
    Then the manifest conversion is rejected before any network request
