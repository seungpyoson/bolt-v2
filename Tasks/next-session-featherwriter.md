# bolt-v2 Next Session: Wire Up FeatherWriter

## Context

Bolt v2 is deployed and trading on a fresh EC2 instance (`i-08dee6aefe9a5b02c`, eu-west-1, c7g.large). ExecTester strategy is running, connected to Polymarket, orders accepted. But no data is being persisted — quotes, trades, and order events flow through NT's message bus and are lost.

## What Needs to Happen

Wire up NT's `FeatherWriter` to subscribe to the message bus and write all events to disk in Apache Arrow/Feather format.

## Key Findings from Research

### The kernel does NOT auto-wire streaming
`NautilusKernel::new()` never calls `config.streaming()`. The `StreamingConfig` field exists on `LiveNodeConfig` but is unused. FeatherWriter must be created manually.

### Feature flags required
- `nautilus-live` needs `features = ["streaming"]` in Cargo.toml
- This pulls in `nautilus-persistence` which contains `FeatherWriter`

### FeatherWriter API
```rust
use nautilus_persistence::backend::feather::FeatherWriter;

let writer = FeatherWriter::new(
    base_path: String,              // e.g. "/opt/bolt-v2/data/catalog"
    store: Arc<dyn ObjectStore>,     // local filesystem
    clock: Rc<RefCell<dyn Clock>>,   // from kernel
    flush_interval_ms: u64,          // e.g. 1000
    replace_existing: bool,          // false
    rotation_config: RotationConfig, // NoRotation or Size/Interval
    per_instrument: HashSet<String>, // which types to split by instrument
);

let handler = FeatherWriter::subscribe_to_message_bus(Rc::new(RefCell::new(writer)))?;
// Now all message bus events get written to feather files
```

### Integration point
After `node = LiveNode::builder(...).build()`, before `node.run()`. The writer needs the kernel's clock, which is accessible via `kernel.clock()`. But LiveNode may not expose the kernel directly — need to check `LiveNode` struct fields.

### Config already exists in live.toml
```toml
[streaming]
enabled = true
catalog_path = "/data/catalog"
flush_interval_ms = 1000
replace_existing = false
```

This section is currently ignored by serde (unknown section). Need to either:
1. Add `StreamingConfig` to our `Config` struct and parse it
2. Or hardcode the values (violates NO HARDCODES rule)

## Changes Required

### Cargo.toml
```toml
nautilus-live = { git = "...", rev = "af2aefc...", features = ["streaming"] }
nautilus-persistence = { git = "...", rev = "af2aefc..." }
```

### src/config.rs
Add streaming config struct to parse `[streaming]` section from TOML.

### src/main.rs
After building the node, create FeatherWriter and subscribe to message bus.

### config/live.toml
`catalog_path` should be `/opt/bolt-v2/data/catalog` (not `/data/catalog` which is the old v1 path).

### EC2 instance
Create `/opt/bolt-v2/data/catalog` directory on the instance.

## Instance Details
- Instance: `i-08dee6aefe9a5b02c` (eu-west-1)
- Binary: `/opt/bolt-v2/bolt-v2`
- Config: `/opt/bolt-v2/config/live.toml`
- Service: `bolt-v2.service`
- Cross-compile: `cargo zigbuild --release --target aarch64-unknown-linux-gnu`
- S3 artifacts: `s3://bolt-deploy-artifacts/artifacts/bolt-v2/`

## Hard Rules
1. NO HARDCODES — streaming config from TOML
2. NO DUAL PATHS — one way to persist data
3. GROUP BY CHANGE — streaming config is its own section, changes together
