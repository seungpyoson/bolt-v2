# OKX Required Config Boundary Design

## Scope

This change removes the PR's case-specific OKX monitor-field guards. It stays
inside issue #1383's official-source correction slice and does not change
provider selection, trading behavior, credentials, or the accepted values of
the OKX monitor controls.

## Problem

The pinned NautilusTrader `OKXDataClientConfig` supplies defaults for three
book-health controls. Bolt must require those controls in TOML so a Nautilus
revision cannot silently change runtime behavior. The current implementation
deserializes the defaulting upstream type, then separately scans raw TOML for
three named keys during validation and mapping. That creates two procedural
guard paths and a field-specific condition list.

## Design

Introduce one Bolt-owned deserialization boundary for OKX data configuration.
It will deserialize the complete TOML table once into a buffered value, require
the three monitor controls through ordinary non-optional typed fields, parse
the same value into the official `OKXDataClientConfig`, and copy the typed
controls into the official config. Missing or invalid controls therefore fail
as schema errors before an upstream config can exist.

Both startup validation and adapter mapping will call the same parser. The
shared data-only provider machinery will receive a required parser function,
using the ordinary upstream deserializer for other providers and the typed OKX
parser for OKX. Parser selection is declared once at the provider entry point;
there is no runtime fallback, alternate value source, or branch on an
individual field.

Delete:

- `OKX_REQUIRED_DATA_FIELDS`;
- `missing_data_fields`;
- `reject_missing_data_fields_for_mapping`;
- the OKX-only raw-TOML checks in validation and mapping.

## Error Behavior

Missing, malformed, or out-of-range required controls produce the existing
configuration/schema error at both public boundaries. The official config is
never constructed with a defaulted value for these controls. Unknown fields
continue to use the existing shared provider-field validation.

## Evidence

- A table-driven test removes each required control and proves both startup
  validation and adapter mapping reject it through the shared typed parser.
- A positive mapping test downcasts the official config and proves all three
  TOML values are preserved exactly.
- Static searches prove the deleted key-list helpers and fallback path do not
  remain.
- Existing non-compile gates, focused remote Rust evidence, and exact-head full
  remote verification cover integration and regression risk before review.

## Non-Goals

- Reimplementing the complete official OKX configuration type.
- Making unrelated upstream optional fields mandatory.
- Removing value-dependent domain decisions; the prohibition is on alternate
  configuration paths and named-case guard ladders, not ordinary validation of
  one typed input.
