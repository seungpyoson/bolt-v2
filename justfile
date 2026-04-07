set shell := ["bash", "-euo", "pipefail", "-c"]

live_input := "config/live.local.toml"
live_input_example := "config/live.local.example.toml"
live_config := "config/live.toml"

live-generate:
    #!/usr/bin/env bash
    if [ ! -f "{{live_input}}" ]; then
        echo "Missing {{live_input}}"
        echo "Create it from {{live_input_example}}, then rerun."
        exit 1
    fi

    cargo run --quiet --bin render_live_config -- --input {{live_input}} --output {{live_config}}

live: live-generate
    cargo run --release --bin bolt-v2 -- run --config {{live_config}}

live-check: live-generate
    cargo run --release --bin bolt-v2 -- secrets check --config {{live_config}}

live-resolve: live-generate
    cargo run --release --bin bolt-v2 -- secrets resolve --config {{live_config}}
