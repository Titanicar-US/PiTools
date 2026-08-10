Feature: PiTools bootstrap

  Scenario: missing credentials fail closed
    Given no PiTools credentials are configured
    When the doctor command loads configuration
    Then configuration validation fails without exposing secret values
