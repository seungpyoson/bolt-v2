# Shadow Mode PnL

Shadow mode measures would-be taker PnL from live data without sending venue orders.

## 1. Run The Live Strategy Without Submitting

Set the strategy parameter:

```toml
[parameters]
submit_orders = false
```

Run the normal bolt-v3 live process. Evaluation, sizing, order-intent evidence, and submit-admission evidence still write to:

```text
<catalog_directory>/<persistence.decision_evidence.order_intents_relative_path>
```

## 2. Join Evidence To Settlement

Prepare a settlement JSONL file from the captured catalog or settlement feed with one row per market/instrument:

```json
{"settlement_date":"2026-06-10","asset":"BTC","market_id":"market-btc","instrument_id":"BTC-UP.POLYMARKET","winning_side":"up","settlement_price":"1.00"}
```

Run:

```bash
python3 scripts/rust_verification.py cargo --repo . -- run --locked --bin shadow_pnl_report -- \
  --evidence-jsonl /var/lib/bolt/catalog/bolt-v3/decision-evidence/order-intents.jsonl \
  --settlements-jsonl /var/lib/bolt/catalog/shadow-settlements.jsonl
```

The output table is grouped by day and asset:

```text
day,asset,would_be_trades,win_rate,gross_pnl,fees,net_pnl,avg_edge_claimed_bps,avg_edge_realized_bps
```
