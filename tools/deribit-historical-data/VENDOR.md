# Vendored: deribit-historical-data (patched)

Vendored copy of **RiveChen/deribit-historical-data** — an async, rate-limit-aware,
resumable Deribit History API v2 trade scraper (futures + options).

- **Upstream:** https://github.com/RiveChen/deribit-historical-data.git
- **Upstream commit:** `24d19c5a9a09b701c1521ea5a035e28ffec9c022`
- **License:** MIT (see `LICENSE`)
- **Why vendored:** the bolt-v2 Apr 29 → Jun 1 2026 Deribit backfill depends on this
  tool plus local patches that exist nowhere upstream. Vendoring guarantees the
  backfill is reproducible without re-cloning a specific upstream commit and
  re-applying patches by hand.

Excluded from the vendor: `.venv/`, `data/` (fetch artifacts), `.git/`,
`__pycache__/`, `*.jsonl`, `*.db`.

## Local patches (delta vs upstream)

1. **`src/deribit_fetcher/config.py`** — new env knobs (all default to upstream behavior):
   - `QUERY_CURRENCY` — the `currency` arg passed to `/get_instruments`. Defaults to `CURRENCY`.
   - `BASE_CURRENCY` — the `base_currency` to keep after enumeration. Defaults to `CURRENCY`.
   - `WINDOW_START_MS` / `WINDOW_END_MS` — epoch-ms instrument-lifetime window filter. Default `None` (no filter).
   - `DATA_ROOT` — output root override; `BASE_DIR = $DATA_ROOT/<CURRENCY>` else `./data/<CURRENCY>`.
     Lets a windowed run write to a fresh root so per-instrument `.done` markers never
     collide with a prior run.
2. **`src/deribit_fetcher/client.py`** — `get_instruments` now filters the enumerated
   instruments by `base_currency == BASE_CURRENCY`, then by lifetime overlap of
   `[WINDOW_START_MS, WINDOW_END_MS)`. Essential for USDC-linear alts (SOL/XRP) which are
   enumerable only via `currency=USDC`.
3. **`src/deribit_fetcher/future.py`, `option.py`** — pass `QUERY_CURRENCY` (not `CURRENCY`)
   to `get_instruments`.
4. **`deribit_window_trades.py`** — NEW standalone window-bounded fetcher (not upstream).
   Pages `GET /get_last_trades_by_instrument_and_time` (start/end epoch-ms, `sorting=asc`,
   `count=1000`, `has_more`), advancing the cursor by trade timestamp (+1ms guard),
   dedups by `trade_id`, keeps half-open `[start, end)`, writes JSONL identical to
   `gen_parquet.py`'s input, atomically publishes `.partial` → final, resumes via a
   per-instrument `<name>.jsonl.done` marker. This fetches ONLY the window — upstream's
   `trade_seq`-walk pulls each instrument's entire lifetime (years / GB for BTC-PERPETUAL).
5. **`deribit_aux_collect.py`** — NEW standalone collector (not upstream) for the
   non-trade option data Deribit's free API serves: `dvol` (DVOL volatility index),
   `settlements` (settlement/delivery prices), `metadata` (option contract definitions,
   read from the window fetcher's `instruments.json`), and `mark_candles` (continuous
   1-min mark-price candles). Uses its own httpx client with **per-endpoint host
   routing** — DVOL/settlements require `www.deribit.com` (the history host 400s them),
   mark candles use `history.deribit.com` — at ≤18 req/s, fail-loud. Writes parquet per
   family/scope. `--family {dvol,settlements,metadata,mark_candles}`.
6. **`deribit_aux_ingest_to_s3.py`** — NEW S3 stager (not upstream) for the auxiliary
   parquet. Reuses the trades-ingest helpers (`stable_json`, content-addressed
   `s3_payload_uri`, fixpoint-hash `manifest_payload`) to stage each family parquet under
   the locked Deribit prefix as `family=deribit_options_auxiliary` with
   `family_kind`/`scope` attrs and a `deribit-options-auxiliary-staging-manifest.v1`
   manifest. Source retained (no delete).

## Reproduce the bolt-v2 Deribit backfill

```sh
# 1) deps (uv reads pyproject.toml + uv.lock)
uv sync

# 2) window-bounded fetch (per asset × kind). SOL/XRP are USDC-linear:
#    BTC/ETH:  CURRENCY=BTC|ETH
#    SOL/XRP:  CURRENCY=SOL|XRP QUERY_CURRENCY=USDC BASE_CURRENCY=SOL|XRP
#    window:   Apr 29 2026 = 1777420800000 .. Jun 1 2026 = 1780272000000
DATA_ROOT=/tmp/deribit-window-data CURRENCY=BTC \
  WINDOW_START_MS=1777420800000 WINDOW_END_MS=1780272000000 \
  uv run python deribit_window_trades.py --kind future   # and --kind option

# 3) merge JSONL -> per-asset/per-kind parquet
DATA_ROOT=/tmp/deribit-window-data CURRENCY=BTC \
  uv run python scripts/gen_parquet.py --type future     # and --type option

# 4) stage merged parquet -> S3 (manifest + content-addressed keys), retain source by default
#    ../../scripts/deribit_rivechen_ingest_to_s3.py  --input-root /tmp/deribit-window-data
```

The S3-staging step (`deribit_rivechen_ingest_to_s3.py`) lives in the repo's `scripts/`,
not in this vendored tool — it consumes this tool's merged parquet output.
