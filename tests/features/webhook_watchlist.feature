Feature: verified webhook intake

  Scenario: invalid signatures cannot reach persistence
    Given a webhook with an invalid X-Hub-Signature-256 header
    When the webhook receiver handles the request
    Then it returns unauthorized before recording an event
