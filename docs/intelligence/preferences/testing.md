# AIOS Preference Testing Strategy

Part of: [preferences.md](../preferences.md) — Preference System
**Related:** [data-model.md](./data-model.md) — Core types under test, [resolution.md](./resolution.md) — Conflict resolution logic, [security.md](./security.md) — Capability enforcement tests, [temporal.md](./temporal.md) — Context rule evaluation tests

-------

## §18 Testing Strategy

The preference system is tested across four layers: pure logic unit tests, cross-component integration tests, property-based invariant verification, and fuzz tests for robustness. QEMU-level validation covers persistence and propagation under realistic boot conditions.

-----

### §18.1 Unit Tests

Unit tests cover each pure-logic component in isolation with no I/O or kernel state.

**PreferenceValue validation** — type checking and constraint enforcement:

```rust
#[test]
fn test_preference_value_bool_rejects_non_bool() { ... }

#[test]
fn test_preference_value_bounded_integer_rejects_out_of_range() { ... }

#[test]
fn test_preference_value_enum_rejects_unlisted_variant() { ... }

#[test]
fn test_preference_value_string_rejects_exceeds_max_length() { ... }
```

**Source precedence ordering** — the seven-level authority ranking must be total and consistent:

```rust
// Authority: EnterpriseLocked > UserExplicit > EnterpriseRecommended >
//            ContextDriven > BehaviorInferred > AgentSuggested > SystemDefault
#[test]
fn test_source_precedence_enterprise_locked_beats_all() { ... }

#[test]
fn test_source_precedence_user_explicit_beats_context_driven() { ... }

#[test]
fn test_source_precedence_agent_suggested_loses_to_behavior_inferred() { ... }

#[test]
fn test_source_precedence_system_default_loses_to_all() { ... }
```

**Conflict resolution logic** — all source × source combinations:

```rust
#[test]
fn test_conflict_resolution_all_source_pairs() {
    // Enumerate every (winner, loser) pair; verify resolve() returns winner's value
    for (a, b) in all_source_pairs() {
        let result = resolve_conflict(pref_with_source(a), pref_with_source(b));
        assert_eq!(result.source, higher_authority(a, b));
    }
}
```

**Context rule condition evaluation** — time-of-day, location, and activity matching:

```rust
#[test]
fn test_context_rule_time_window_matches_within_range() { ... }

#[test]
fn test_context_rule_time_window_no_match_outside_range() { ... }

#[test]
fn test_context_rule_location_matches_known_zone() { ... }

#[test]
fn test_context_rule_activity_focus_mode_activates_rule() { ... }

#[test]
fn test_context_rule_all_conditions_must_match() { ... }
```

**Schema validation** — valid and invalid values against each schema type:

```rust
#[test]
fn test_schema_validates_correct_enum_value() { ... }

#[test]
fn test_schema_rejects_value_wrong_type_for_key() { ... }

#[test]
fn test_schema_required_field_missing_fails_validation() { ... }
```

**Rate limiter enforcement** — per-hour and per-day limits, cooldown after rejection:

```rust
#[test]
fn test_rate_limiter_blocks_after_hourly_quota_exhausted() { ... }

#[test]
fn test_rate_limiter_cooldown_prevents_immediate_retry() { ... }

#[test]
fn test_rate_limiter_resets_after_window_expires() { ... }
```

**Capability attenuation logic** — category, prefix, and specific-preference scoping:

```rust
#[test]
fn test_capability_category_scope_allows_any_key_in_category() { ... }

#[test]
fn test_capability_prefix_scope_rejects_key_outside_prefix() { ... }

#[test]
fn test_capability_specific_scope_allows_only_exact_key() { ... }

#[test]
fn test_attenuated_write_capability_cannot_read() { ... }
```

-----

### §18.2 Integration Tests

Integration tests verify correct behavior across component boundaries with a running kernel and preference service.

**NLU → Preference Service → Propagation → Component subscription** (end-to-end flow):

```rust
#[test]
fn test_nlu_utterance_propagates_to_subscribed_component() {
    // Subscriber registers for "display.brightness"
    // NLU resolves "make it dimmer" → brightness -= 20
    // Preference service updates value and source = UserExplicit { ConversationBar }
    // Component receives ChangeNotification with new value
}
```

**Agent manifest → capability gate → preference read/write** (permission enforcement):

```rust
#[test]
fn test_agent_without_write_capability_cannot_set_preference() { ... }

#[test]
fn test_agent_with_category_capability_can_write_any_key_in_category() { ... }

#[test]
fn test_agent_capability_revocation_immediately_blocks_subsequent_writes() { ... }
```

**Context change → rule evaluation → preference override → propagation** (temporal/contextual flow):

```rust
#[test]
fn test_focus_mode_activation_overrides_notification_preference() {
    // context.activity = FocusMode
    // Rule: if activity == FocusMode then notifications.sound = Silent
    // Verify: preference service holds ContextDriven override
    // Verify: audio subsystem notified
    // context.activity = Idle → override removed → UserExplicit value restored
}
```

**Enterprise policy → signature verification → preference lock → Settings UI shows locked state**:

```rust
#[test]
fn test_enterprise_policy_locks_preference_and_settings_ui_reflects_locked() { ... }

#[test]
fn test_tampered_enterprise_policy_rejected_by_signature_check() { ... }
```

**Cross-device sync** — change on device A applied on device B:

```rust
#[test]
fn test_cross_device_sync_propagates_user_explicit_change() {
    // Device A: set("display.theme", "dark", UserExplicit)
    // Space Mesh delta broadcast
    // Device B: receives delta, applies, source = UserExplicit
    // Device B: UserExplicit value matches device A value
}
```

**History** — set → change → explain → undo → verify restored value:

```rust
#[test]
fn test_undo_restores_previous_value_after_single_change() { ... }

#[test]
fn test_explain_returns_current_source_and_value_after_undo() { ... }
```

-----

### §18.3 Property-Based Tests

Property-based tests use randomized inputs to verify system-wide invariants. All properties hold for any valid sequence of operations.

**Explain reflects current state** — for any sequence of preference changes, `explain()` always returns the current source and value:

```text
forall key, operations: [PreferenceOp]:
  apply_all(operations)
  result = explain(key)
  assert result.value == current_value(key)
  assert result.source == current_source(key)
```

**Conflict resolution is deterministic** — same inputs always produce the same output:

```text
forall a: PreferenceEntry, b: PreferenceEntry:
  resolve_conflict(a, b) == resolve_conflict(a, b)
```

**Undo is symmetric** — `set(X)` followed by `undo()` exactly restores the previous value:

```text
forall key, v_before, v_after:
  prev = get(key)  // = v_before
  set(key, v_after, UserExplicit)
  undo()
  assert get(key) == prev
```

**Authority ranking is total** — for any two distinct sources, one always dominates:

```text
forall a: PreferenceSource, b: PreferenceSource, a != b:
  (authority(a) > authority(b)) XOR (authority(b) > authority(a))
```

**Schema validation is sound** — a value that passes validation can be stored and retrieved identically:

```text
forall key, value:
  if validate(key, value) == Ok:
    set(key, value, UserExplicit)
    assert get(key) == value
```

**Rate limiting is monotonic** — more suggestions never decrease the rejection probability:

```text
forall agent, key, n > m:
  suggestions_rejected_after_n >= suggestions_rejected_after_m
```

-----

### §18.4 Fuzz Tests

Fuzz tests drive the preference system with malformed, random, or adversarial inputs to confirm no panics, memory unsafety, or logic violations occur.

**Random PreferenceValue payloads against schema validation** — the validator must not panic on any input:

```rust
fuzz_target!(|data: &[u8]| {
    let _ = PreferenceValue::from_bytes(data);  // must not panic
});
```

**Malformed NLU input strings against the NLU resolver**:

```rust
fuzz_target!(|data: &[u8]| {
    if let Ok(s) = core::str::from_utf8(data) {
        let _ = NluResolver::resolve(s);  // must return Err, not panic
    }
});
```

**Concurrent preference changes from multiple agents** — races must not corrupt state:

```rust
// Spawn N agents each hammering the same key with random values
// Verify final state is a valid entry from one of the agents (no torn writes)
fn fuzz_concurrent_writes(agent_count: usize, iterations: usize) { ... }
```

**Random context rule activation/deactivation sequences** — rule engine must remain consistent:

```rust
fuzz_target!(|ops: Vec<ContextRuleOp>| {
    for op in ops { apply_rule_op(op); }
    assert!(rule_engine_is_consistent());
});
```

**Invalid enterprise policy signatures** — the policy loader must reject without panicking:

```rust
fuzz_target!(|data: &[u8]| {
    let result = EnterprisePolicy::load_and_verify(data);
    assert!(result.is_err());  // unsigned/malformed policies always rejected
});
```

-----

### §18.5 QEMU Validation

System-level tests run inside QEMU to validate preference behavior across the full boot sequence and in realistic runtime conditions.

**Preference persistence across reboot** — set preference, reboot, verify value restored from Space storage:

```text
Step 1: set("display.theme", "dark", UserExplicit)
Step 2: graceful shutdown
Step 3: QEMU reboot
Step 4: verify get("display.theme") == "dark", source == UserExplicit
UART output: "pref: restored 1 user preferences from Space"
```

**Preference propagation timing** — measure latency from `set()` to component `ChangeNotification`:

```text
Acceptance: p99 propagation latency < 5 ms under single-agent workload
Measured via: TICK_COUNT delta between set() and notification delivery
```

**Cross-device sync simulation** — two QEMU instances syncing preferences via Space Mesh:

```text
Instance A: set("input.keyboard.repeat_delay_ms", 300, UserExplicit)
Space Mesh: delta serialized and delivered to instance B
Instance B: verify get("input.keyboard.repeat_delay_ms") == 300
UART output (B): "pref: sync applied 1 delta(s) from peer"
```

**Enterprise policy boot sequence** — MDM policy applied before user preferences loaded:

```text
Step 1: MDM policy configures EnterpriseLocked("network.vpn.required", true)
Step 2: QEMU boot with policy in ESP
Step 3: verify "network.vpn.required" == true, source == EnterpriseLocked
Step 4: user attempts set("network.vpn.required", false) → returns Err(Locked)
UART output: "pref: enterprise policy applied 1 locked preference(s)"
```
