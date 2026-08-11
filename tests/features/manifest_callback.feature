Feature: GitHub App manifest setup

  Scenario: unsafe manifest conversion codes fail closed
    Given an unsafe GitHub App manifest conversion code
    When PiTools validates the manifest conversion code
    Then the manifest conversion is rejected before any network request

  Scenario: the public manifest can read protected branch requirements
    Given the public GitHub App manifest
    When PiTools validates its default permissions
    Then the manifest requests administration read permission
