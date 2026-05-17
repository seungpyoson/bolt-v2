# Data Model: CI Slow Test Post-Stop Delay

## FastTestLiveNode

Fields:

- `trader_id`: fixed test trader `TESTER-001`
- `environment`: `Environment::Live`
- `delay_post_stop_secs`: `0`

Validation:

- Builds through NautilusTrader `LiveNode::builder`.
- Does not alter production config structs.
- Used only by tests that do not assert post-stop drain delay.

## RuntimeEvidence

Fields:

- `command`
- `before_duration`
- `before_log`
- `after_duration`
- `after_log`

Validation:

- Evidence comes from the same representative test command.
- The after run still passes the test.
