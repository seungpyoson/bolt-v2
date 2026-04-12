# Issue #118 Runtime Fallback Contract

This note documents only the executable-book and FOK boundary inside `#118`.
It does not restate the other accepted runtime-enablement work already landed in this branch
such as candidate metadata, eligible-set handoff, and no-preemption safety.

`#118` does not claim native Polymarket `order_book_depths` support.

The accepted runtime path for executable-book entry in this slice is:

- keep [`contracts/polymarket.toml`](../contracts/polymarket.toml) truthful with `order_book_depths = unsupported`
- let runtime-managed strategies use the shared `exec_tester` seam with `subscribe_book = true`
- rely on the pinned Nautilus Trader snapshot-at-interval wrapper, which forwards book snapshots to native `order_book_deltas` subscriptions and builds the order-book view internally
- use `open_position_time_in_force = "Fok"` on the runtime-managed opening market order when FOK entry semantics are required

What this means for downstream strategy work:

- `#110` can depend on the runtime-managed fallback above
- `#110` must not assume venue-level `order_book_depths` support exists

What remains outside `#118`:

- changing the venue contract to advertise true Polymarket depth support
- any broader platform work needed to make that contract claim truthful end to end
