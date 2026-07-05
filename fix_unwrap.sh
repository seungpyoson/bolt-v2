#!/bin/bash
# Replaces unwrap_or(Decimal::ZERO) with proper fallback elimination
sed -i 's/\.unwrap_or(Decimal::ZERO)/.expect("Capital admission state missing")/g' src/bolt_v3_submit_admission.rs
