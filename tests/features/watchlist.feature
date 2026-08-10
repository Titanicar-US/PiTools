Feature: PR watchlist state

  Scenario: a verified pull request event enters the watchlist
    Given a GitHub pull request delivery with a valid signature
    When PiTools records the delivery
    Then the pull request is watched until it is closed or merged

  Scenario: duplicate deliveries are harmless
    Given the same GitHub delivery ID is received twice
    When PiTools records both deliveries
    Then only one event is processed

  Scenario: suspended installations stop reconciliation
    Given a GitHub App installation suspension delivery
    When PiTools classifies the installation lifecycle
    Then reconciliation is deactivated for the installation

  Scenario: removed repositories stop reconciliation
    Given a GitHub App repository removal delivery
    When PiTools classifies the installation lifecycle
    Then reconciliation is deactivated for the repository
