use std::{
    collections::{BTreeMap, HashMap},
    fmt::{self, Display, Formatter, Write as FmtWrite},
    fs::File,
    io::{BufRead, BufReader, Write},
    path::Path,
};

use anyhow::{Context, Result, anyhow};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    EntryOrderIntentFact, ShadowPnlEvent, SubmitLinkedStrategyInputSnapshotFact,
    read_shadow_pnl_events,
};
use crate::bolt_v3_market_families::OutcomeSide;
use crate::bolt_v3_taker_updown_signal::outcome_side_evidence_label;

const SHADOW_PNL_COUNT_INCREMENT: u64 = 1;
const SHADOW_PNL_LINE_NUMBER_BASE: usize = 1;
const SHADOW_PNL_BASIS_POINTS_DENOMINATOR: u64 = crate::bolt_v3_numeric::BPS_DENOMINATOR as u64;
const SHADOW_PNL_DECIMAL_SCALE: u32 = 6;
const SHADOW_PNL_CSV_SEPARATOR: char = ',';
const SHADOW_PNL_CSV_QUOTE: char = '"';
const SHADOW_PNL_CSV_LINE_FEED: char = '\n';
const SHADOW_PNL_CSV_CARRIAGE_RETURN: char = '\r';

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShadowPnlReportRow {
    pub day: NaiveDate,
    pub asset: String,
    pub would_be_trades: u64,
    pub win_rate: String,
    pub gross_pnl: String,
    pub fees: String,
    pub net_pnl: String,
    pub avg_edge_claimed_bps: String,
    pub avg_edge_realized_bps: String,
}

impl Display for ShadowPnlReportRow {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.day)?;
        f.write_char(SHADOW_PNL_CSV_SEPARATOR)?;
        fmt_shadow_pnl_csv_field(f, &self.asset)?;
        f.write_char(SHADOW_PNL_CSV_SEPARATOR)?;
        write!(f, "{}", self.would_be_trades)?;
        f.write_char(SHADOW_PNL_CSV_SEPARATOR)?;
        fmt_shadow_pnl_csv_field(f, &self.win_rate)?;
        f.write_char(SHADOW_PNL_CSV_SEPARATOR)?;
        fmt_shadow_pnl_csv_field(f, &self.gross_pnl)?;
        f.write_char(SHADOW_PNL_CSV_SEPARATOR)?;
        fmt_shadow_pnl_csv_field(f, &self.fees)?;
        f.write_char(SHADOW_PNL_CSV_SEPARATOR)?;
        fmt_shadow_pnl_csv_field(f, &self.net_pnl)?;
        f.write_char(SHADOW_PNL_CSV_SEPARATOR)?;
        fmt_shadow_pnl_csv_field(f, &self.avg_edge_claimed_bps)?;
        f.write_char(SHADOW_PNL_CSV_SEPARATOR)?;
        fmt_shadow_pnl_csv_field(f, &self.avg_edge_realized_bps)
    }
}

fn fmt_shadow_pnl_csv_field(f: &mut Formatter<'_>, value: &str) -> fmt::Result {
    if !shadow_pnl_csv_field_requires_quotes(value) {
        return f.write_str(value);
    }

    f.write_char(SHADOW_PNL_CSV_QUOTE)?;
    for character in value.chars() {
        if character == SHADOW_PNL_CSV_QUOTE {
            f.write_char(SHADOW_PNL_CSV_QUOTE)?;
        }
        f.write_char(character)?;
    }
    f.write_char(SHADOW_PNL_CSV_QUOTE)
}

fn shadow_pnl_csv_field_requires_quotes(value: &str) -> bool {
    value.chars().any(|character| {
        matches!(
            character,
            SHADOW_PNL_CSV_SEPARATOR
                | SHADOW_PNL_CSV_QUOTE
                | SHADOW_PNL_CSV_LINE_FEED
                | SHADOW_PNL_CSV_CARRIAGE_RETURN
        )
    })
}

#[derive(Debug, Clone, Default)]
struct TradeAccumulator {
    trades: u64,
    wins: u64,
    gross_pnl: Decimal,
    fees: Decimal,
    claimed_edge_bps: Decimal,
    realized_edge_bps: Decimal,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShadowSettlementEvidence {
    pub settlement_date: NaiveDate,
    pub asset: String,
    pub market_id: Option<String>,
    pub instrument_id: String,
    pub winning_side: String,
    pub settlement_price: String,
}

#[derive(Debug, Clone)]
struct TradeEvidence {
    snapshot: SubmitLinkedStrategyInputSnapshotFact,
    intent: EntryOrderIntentFact,
}

pub fn build_shadow_pnl_report(
    evidence_jsonl: &Path,
    settlements_jsonl: &Path,
    evidence_max_bytes: u64,
) -> Result<Vec<ShadowPnlReportRow>> {
    let chains = read_admitted_entry_chains(evidence_jsonl, evidence_max_bytes)?;
    let settlements = read_settlements(settlements_jsonl)?;
    let mut accumulators = BTreeMap::<(NaiveDate, String), TradeAccumulator>::new();

    for trade in chains {
        let settlement = settlement_for_trade(&settlements, &trade)?;
        let selected_side = trade
            .snapshot
            .details
            .selected_side
            .as_deref()
            .ok_or_else(|| {
                anyhow!(
                    "missing selected_side for {}",
                    trade.intent.details.client_order_id
                )
            })?;
        if !outcome_side_is_recognized(selected_side) {
            return Err(anyhow!(
                "evidence selected_side {:?} for {} is invalid: not a recognized binary outcome side",
                selected_side,
                trade.intent.details.client_order_id
            ));
        }
        let entry_price = parse_decimal(&trade.intent.details.price)?;
        let quantity = parse_decimal(&trade.intent.details.quantity)?;
        let settlement_price = parse_decimal(&settlement.settlement_price)?;
        let fee_bps = parse_decimal(&trade.snapshot.details.fee_rate_basis_points)?;
        let claimed_edge = parse_decimal(&trade.snapshot.details.expected_edge_basis_points)?;
        let notional = entry_price * quantity;
        let gross = (settlement_price - entry_price) * quantity;
        let fees = notional * fee_bps / Decimal::from(SHADOW_PNL_BASIS_POINTS_DENOMINATOR);
        let realized_edge = if notional.is_zero() {
            Decimal::ZERO
        } else {
            gross / notional * Decimal::from(SHADOW_PNL_BASIS_POINTS_DENOMINATOR)
        };
        // winning_side is operator-prepared free text. Validate it names a legal
        // binary outcome side BEFORE deriving won, so a malformed/typo'd value
        // fails loud here instead of silently collapsing to won=false (and being
        // counted as a loss) just because a garbage string never equals the
        // selected side.
        if !outcome_side_is_recognized(&settlement.winning_side) {
            return Err(anyhow!(
                "settlement winning_side {:?} for {} is invalid: not a recognized binary outcome side",
                settlement.winning_side,
                trade.intent.details.client_order_id
            ));
        }
        let won = selected_side.eq_ignore_ascii_case(settlement.winning_side.as_str());
        // winning_side (which side resolved) and settlement_price (the realized
        // payout) describe the SAME settlement and must agree. Fail loud on operator
        // data where they contradict, rather than emit a self-inconsistent row (a
        // counted win with negative PnL, or a counted loss with positive PnL).
        if won && settlement_price < entry_price {
            return Err(anyhow!(
                "settlement inconsistency for {}: winning_side {} marks a win but settlement_price {settlement_price} is below entry_price {entry_price}",
                trade.intent.details.client_order_id,
                settlement.winning_side
            ));
        }
        if !won && settlement_price > entry_price {
            return Err(anyhow!(
                "settlement inconsistency for {}: winning_side {} marks a loss but settlement_price {settlement_price} is above entry_price {entry_price}",
                trade.intent.details.client_order_id,
                settlement.winning_side
            ));
        }

        let accumulator = accumulators
            .entry((settlement.settlement_date, settlement.asset.clone()))
            .or_default();
        accumulator.trades += SHADOW_PNL_COUNT_INCREMENT;
        if won {
            accumulator.wins += SHADOW_PNL_COUNT_INCREMENT;
        }
        accumulator.gross_pnl += gross;
        accumulator.fees += fees;
        accumulator.claimed_edge_bps += claimed_edge;
        accumulator.realized_edge_bps += realized_edge;
    }

    Ok(accumulators
        .into_iter()
        .map(|((day, asset), accumulator)| report_row(day, asset, accumulator))
        .collect())
}

pub fn write_shadow_pnl_csv(rows: &[ShadowPnlReportRow], writer: &mut impl Write) -> Result<()> {
    write_shadow_pnl_csv_header(writer)?;
    for row in rows {
        writeln!(writer, "{row}").context("failed to write shadow PnL row")?;
    }
    Ok(())
}

fn write_shadow_pnl_csv_header(writer: &mut impl Write) -> Result<()> {
    write!(writer, "day")?;
    write!(writer, ",asset")?;
    write!(writer, ",would_be_trades")?;
    write!(writer, ",win_rate")?;
    write!(writer, ",gross_pnl")?;
    write!(writer, ",fees")?;
    write!(writer, ",net_pnl")?;
    write!(writer, ",avg_edge_claimed_bps")?;
    writeln!(writer, ",avg_edge_realized_bps")?;
    Ok(())
}

fn read_admitted_entry_chains(path: &Path, evidence_max_bytes: u64) -> Result<Vec<TradeEvidence>> {
    let mut snapshots = HashMap::<String, SubmitLinkedStrategyInputSnapshotFact>::new();
    let mut intents = HashMap::<String, EntryOrderIntentFact>::new();
    let mut admitted_entries = HashMap::<String, ()>::new();

    for (event_index, event) in read_shadow_pnl_events(path, evidence_max_bytes)?
        .into_iter()
        .enumerate()
    {
        match event {
            ShadowPnlEvent::SubmitLinkedStrategyInputSnapshot(snapshot) => {
                let client_order_id = snapshot.submission.client_order_id.clone();
                insert_unique_evidence(
                    &mut snapshots,
                    client_order_id,
                    *snapshot,
                    "submit-linked strategy-input snapshot",
                    event_index + SHADOW_PNL_LINE_NUMBER_BASE,
                )?;
            }
            ShadowPnlEvent::EntryOrderIntent(intent) => {
                let client_order_id = intent.details.client_order_id.clone();
                insert_unique_evidence(
                    &mut intents,
                    client_order_id,
                    intent,
                    "entry order intent",
                    event_index + SHADOW_PNL_LINE_NUMBER_BASE,
                )?;
            }
            ShadowPnlEvent::AdmittedEntryAdmission(admission) => {
                let client_order_id = admission.details.client_order_id;
                insert_unique_evidence(
                    &mut admitted_entries,
                    client_order_id,
                    (),
                    "admitted entry admission",
                    event_index + SHADOW_PNL_LINE_NUMBER_BASE,
                )?;
            }
        }
    }

    // Drive reconstruction from the admitted entries: every admitted entry is a
    // would-be trade and MUST carry both its order intent and its input snapshot.
    // Iterating intents instead would silently drop an admitted entry whose intent
    // line is missing or corrupted; here a missing intent or snapshot fails loud.
    let mut chains = Vec::new();
    for client_order_id in admitted_entries.into_keys() {
        let intent = intents
            .get(&client_order_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing order intent for admitted entry {client_order_id}"))?;
        let snapshot = snapshots.get(&client_order_id).cloned().ok_or_else(|| {
            anyhow!("missing strategy input snapshot for admitted entry {client_order_id}")
        })?;
        chains.push(TradeEvidence { snapshot, intent });
    }
    chains.sort_by(|left, right| {
        left.intent
            .details
            .client_order_id
            .cmp(&right.intent.details.client_order_id)
    });
    Ok(chains)
}

/// Insert decision evidence keyed by client_order_id, failing loud on a duplicate.
///
/// The settlement join treats client_order_id as the unique identity of a
/// would-be trade. Decision evidence accumulates in append-mode JSONL across
/// process runs, so a reused client_order_id (e.g. a non-UUID id scheme after a
/// restart) would otherwise silently overwrite an earlier would-be trade. Reject
/// the ambiguity instead of dropping a trade.
fn insert_unique_evidence<V>(
    map: &mut HashMap<String, V>,
    client_order_id: String,
    value: V,
    record_kind: &str,
    line_number: usize,
) -> Result<()> {
    if map.insert(client_order_id.clone(), value).is_some() {
        return Err(anyhow!(
            "duplicate {record_kind} decision evidence for client_order_id {client_order_id} at line {line_number}; cannot disambiguate would-be trades"
        ));
    }
    Ok(())
}

fn read_settlements(path: &Path) -> Result<Vec<ShadowSettlementEvidence>> {
    read_jsonl_lines(path)?
        .into_iter()
        .map(|(line_number, line)| {
            serde_json::from_str(&line).with_context(|| {
                format!(
                    "failed to parse settlement line {line_number} in {}",
                    path.display()
                )
            })
        })
        .collect()
}

fn read_jsonl_lines(path: &Path) -> Result<Vec<(usize, String)>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    BufReader::new(file)
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_number = index + SHADOW_PNL_LINE_NUMBER_BASE;
            match line {
                Ok(line) if line.trim().is_empty() => None,
                Ok(line) => Some(Ok((line_number, line))),
                Err(error) => Some(Err(anyhow!(
                    "failed to read line {line_number} in {}: {error}",
                    path.display()
                ))),
            }
        })
        .collect()
}

fn settlement_for_trade<'a>(
    settlements: &'a [ShadowSettlementEvidence],
    trade: &TradeEvidence,
) -> Result<&'a ShadowSettlementEvidence> {
    let instrument_matches = settlements
        .iter()
        .filter(|settlement| settlement.instrument_id == trade.intent.details.instrument_id)
        .collect::<Vec<_>>();
    let client_order_id = trade.intent.details.client_order_id.as_str();
    let Some(market_id) = trade.snapshot.details.market_id.as_ref() else {
        return single_settlement_match(
            instrument_matches,
            format!("ambiguous settlement for {client_order_id}: missing trade market_id"),
        )?
        .ok_or_else(|| anyhow!("missing settlement for {client_order_id}"));
    };
    let exact_matches = instrument_matches
        .iter()
        .copied()
        .filter(|settlement| settlement.market_id.as_ref() == Some(market_id))
        .collect::<Vec<_>>();
    if let Some(settlement) = single_settlement_match(
        exact_matches,
        format!("ambiguous settlement for {client_order_id}: duplicate market_id match"),
    )? {
        return Ok(settlement);
    }
    let wildcard_matches = instrument_matches
        .into_iter()
        .filter(|settlement| settlement.market_id.is_none())
        .collect::<Vec<_>>();
    single_settlement_match(
        wildcard_matches,
        format!("ambiguous settlement for {client_order_id}: duplicate instrument-only match"),
    )?
    .ok_or_else(|| anyhow!("missing settlement for {client_order_id}"))
}

fn single_settlement_match(
    settlements: Vec<&ShadowSettlementEvidence>,
    ambiguous_message: String,
) -> Result<Option<&ShadowSettlementEvidence>> {
    match settlements.len() {
        0 => Ok(None),
        1 => Ok(Some(settlements[0])),
        _ => Err(anyhow!(ambiguous_message)),
    }
}

/// Whether an evidence or operator-provided side token names a legal binary
/// outcome side. The legal vocabulary is sourced from the canonical
/// [`OutcomeSide`] enum via its evidence label, so this report never re-defines
/// the side strings and rejects any unrecognized token loudly.
fn outcome_side_is_recognized(candidate: &str) -> bool {
    [OutcomeSide::Up, OutcomeSide::Down]
        .into_iter()
        .any(|side| outcome_side_evidence_label(side).eq_ignore_ascii_case(candidate))
}

fn report_row(day: NaiveDate, asset: String, accumulator: TradeAccumulator) -> ShadowPnlReportRow {
    let trades = Decimal::from(accumulator.trades);
    let win_rate = Decimal::from(accumulator.wins) / trades;
    let net_pnl = accumulator.gross_pnl - accumulator.fees;
    ShadowPnlReportRow {
        day,
        asset,
        would_be_trades: accumulator.trades,
        win_rate: format_decimal(win_rate),
        gross_pnl: format_decimal(accumulator.gross_pnl),
        fees: format_decimal(accumulator.fees),
        net_pnl: format_decimal(net_pnl),
        avg_edge_claimed_bps: format_decimal(accumulator.claimed_edge_bps / trades),
        avg_edge_realized_bps: format_decimal(accumulator.realized_edge_bps / trades),
    }
}

fn parse_decimal(raw: &str) -> Result<Decimal> {
    raw.parse::<Decimal>().context("invalid decimal value")
}

fn format_decimal(value: Decimal) -> String {
    value
        .round_dp(SHADOW_PNL_DECIMAL_SCALE)
        .normalize()
        .to_string()
}
