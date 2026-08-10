Feature: PiTools bootstrap

  Scenario: missing credentials fail closed
    Given no PiTools credentials are configured
    When the doctor command loads configuration
    Then configuration validation fails without exposing secret values

  Scenario: operator commands are discoverable without credentials
    Given no PiTools credentials are configured
    When the operator reads the CLI help
    Then the CLI exposes bounded inspection and cancellation commands

  Scenario: doctor reports all GitHub App installations
    Given a GitHub App is authenticated with selected installations
    When the doctor renders GitHub App status
    Then the doctor reports the App identity and installation count
