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

use crate::bolt_v3_decision_evidence::{
    BOLT_V3_ADMISSION_DECISION_RECORD_KIND, BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
    BOLT_V3_ORDER_INTENT_GATE_ID, BOLT_V3_ORDER_INTENT_RECORD_KIND,
    BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID, BOLT_V3_STRATEGY_INPUT_SNAPSHOT_RECORD_KIND,
    BOLT_V3_SUBMIT_ADMISSION_GATE_ID, BoltV3AdmissionOutcome, BoltV3OrderIntentKind,
    BoltV3SubmitIntentKind,
};

const SHADOW_PNL_COUNT_INCREMENT: u64 = 1;
const SHADOW_PNL_LINE_NUMBER_BASE: usize = 1;
const SHADOW_PNL_BASIS_POINTS_DENOMINATOR: u64 = 10_000;
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

#[derive(Debug, Deserialize)]
struct EvidenceEnvelope {
    schema_version: u32,
    gate_id: String,
    kind: String,
    snapshot: Option<StrategyInputSnapshotEvidence>,
    intent: Option<OrderIntentEvidence>,
    decision: Option<AdmissionDecisionEvidence>,
}

#[derive(Debug, Clone, Deserialize)]
struct StrategyInputSnapshotEvidence {
    market_id: Option<String>,
    selected_side: Option<String>,
    expected_edge_basis_points: String,
    fee_rate_basis_points: String,
    client_order_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct OrderIntentEvidence {
    intent_kind: BoltV3OrderIntentKind,
    instrument_id: String,
    client_order_id: String,
    price: String,
    quantity: String,
}

#[derive(Debug, Clone, Deserialize)]
struct AdmissionDecisionEvidence {
    client_order_id: String,
    intent_kind: BoltV3SubmitIntentKind,
    outcome: BoltV3AdmissionOutcome,
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
    snapshot: StrategyInputSnapshotEvidence,
    intent: OrderIntentEvidence,
}

pub fn build_shadow_pnl_report(
    evidence_jsonl: &Path,
    settlements_jsonl: &Path,
) -> Result<Vec<ShadowPnlReportRow>> {
    let chains = read_admitted_entry_chains(evidence_jsonl)?;
    let settlements = read_settlements(settlements_jsonl)?;
    let mut accumulators = BTreeMap::<(NaiveDate, String), TradeAccumulator>::new();

    for trade in chains {
        let settlement = settlement_for_trade(&settlements, &trade)
            .with_context(|| format!("missing settlement for {}", trade.intent.client_order_id))?;
        let selected_side =
            trade.snapshot.selected_side.as_deref().ok_or_else(|| {
                anyhow!("missing selected_side for {}", trade.intent.client_order_id)
            })?;
        let entry_price = parse_decimal(&trade.intent.price)?;
        let quantity = parse_decimal(&trade.intent.quantity)?;
        let settlement_price = parse_decimal(&settlement.settlement_price)?;
        let fee_bps = parse_decimal(&trade.snapshot.fee_rate_basis_points)?;
        let claimed_edge = parse_decimal(&trade.snapshot.expected_edge_basis_points)?;
        let notional = entry_price * quantity;
        let gross = (settlement_price - entry_price) * quantity;
        let fees = notional * fee_bps / Decimal::from(SHADOW_PNL_BASIS_POINTS_DENOMINATOR);
        let realized_edge = if notional.is_zero() {
            Decimal::ZERO
        } else {
            gross / notional * Decimal::from(SHADOW_PNL_BASIS_POINTS_DENOMINATOR)
        };
        let won = selected_side.eq_ignore_ascii_case(settlement.winning_side.as_str());

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

fn read_admitted_entry_chains(path: &Path) -> Result<Vec<TradeEvidence>> {
    let mut snapshots = HashMap::<String, StrategyInputSnapshotEvidence>::new();
    let mut intents = HashMap::<String, OrderIntentEvidence>::new();
    let mut admitted_entries = HashMap::<String, AdmissionDecisionEvidence>::new();

    for (line_index, line) in read_jsonl_lines(path)?.into_iter().enumerate() {
        let envelope: EvidenceEnvelope = serde_json::from_str(&line).with_context(|| {
            format!(
                "failed to parse decision evidence line {} in {}",
                line_index + SHADOW_PNL_LINE_NUMBER_BASE,
                path.display()
            )
        })?;
        validate_evidence_header(&envelope, line_index + SHADOW_PNL_LINE_NUMBER_BASE)?;
        match envelope.kind.as_str() {
            BOLT_V3_STRATEGY_INPUT_SNAPSHOT_RECORD_KIND => {
                let snapshot = envelope.snapshot.ok_or_else(|| {
                    anyhow!(
                        "missing snapshot payload at decision evidence line {}",
                        line_index + SHADOW_PNL_LINE_NUMBER_BASE
                    )
                })?;
                snapshots.insert(snapshot.client_order_id.clone(), snapshot);
            }
            BOLT_V3_ORDER_INTENT_RECORD_KIND => {
                let intent = envelope.intent.ok_or_else(|| {
                    anyhow!(
                        "missing intent payload at decision evidence line {}",
                        line_index + SHADOW_PNL_LINE_NUMBER_BASE
                    )
                })?;
                if intent.intent_kind == BoltV3OrderIntentKind::Entry {
                    intents.insert(intent.client_order_id.clone(), intent);
                }
            }
            BOLT_V3_ADMISSION_DECISION_RECORD_KIND => {
                let decision = envelope.decision.ok_or_else(|| {
                    anyhow!(
                        "missing admission decision payload at decision evidence line {}",
                        line_index + SHADOW_PNL_LINE_NUMBER_BASE
                    )
                })?;
                if decision.intent_kind == BoltV3SubmitIntentKind::Entry
                    && decision.outcome == BoltV3AdmissionOutcome::Admitted
                {
                    admitted_entries.insert(decision.client_order_id.clone(), decision);
                }
            }
            _ => {}
        }
    }

    let mut chains = Vec::new();
    for (client_order_id, intent) in intents {
        if !admitted_entries.contains_key(&client_order_id) {
            continue;
        }
        let snapshot = snapshots.get(&client_order_id).cloned().ok_or_else(|| {
            anyhow!("missing strategy input snapshot for admitted entry {client_order_id}")
        })?;
        chains.push(TradeEvidence { snapshot, intent });
    }
    chains.sort_by(|left, right| {
        left.intent
            .client_order_id
            .cmp(&right.intent.client_order_id)
    });
    Ok(chains)
}

fn read_settlements(path: &Path) -> Result<Vec<ShadowSettlementEvidence>> {
    read_jsonl_lines(path)?
        .into_iter()
        .enumerate()
        .map(|(line_index, line)| {
            serde_json::from_str(&line).with_context(|| {
                format!(
                    "failed to parse settlement line {} in {}",
                    line_index + SHADOW_PNL_LINE_NUMBER_BASE,
                    path.display()
                )
            })
        })
        .collect()
}

fn read_jsonl_lines(path: &Path) -> Result<Vec<String>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    BufReader::new(file)
        .lines()
        .enumerate()
        .filter_map(|(index, line)| match line {
            Ok(line) if line.trim().is_empty() => None,
            Ok(line) => Some(Ok(line)),
            Err(error) => Some(Err(anyhow!(
                "failed to read line {} in {}: {error}",
                index + SHADOW_PNL_LINE_NUMBER_BASE,
                path.display()
            ))),
        })
        .collect()
}

fn validate_evidence_header(envelope: &EvidenceEnvelope, line_number: usize) -> Result<()> {
    if envelope.schema_version != BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION {
        return Err(anyhow!(
            "invalid decision evidence schema_version at line {line_number}"
        ));
    }
    let expected_gate_id = match envelope.kind.as_str() {
        BOLT_V3_STRATEGY_INPUT_SNAPSHOT_RECORD_KIND => BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID,
        BOLT_V3_ORDER_INTENT_RECORD_KIND => BOLT_V3_ORDER_INTENT_GATE_ID,
        BOLT_V3_ADMISSION_DECISION_RECORD_KIND => BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
        _ => return Ok(()),
    };
    if envelope.gate_id != expected_gate_id {
        return Err(anyhow!(
            "invalid decision evidence gate_id at line {line_number}"
        ));
    }
    Ok(())
}

fn settlement_for_trade<'a>(
    settlements: &'a [ShadowSettlementEvidence],
    trade: &TradeEvidence,
) -> Option<&'a ShadowSettlementEvidence> {
    settlements
        .iter()
        .find(|settlement| settlement_matches_trade(settlement, trade))
}

fn settlement_matches_trade(settlement: &ShadowSettlementEvidence, trade: &TradeEvidence) -> bool {
    if settlement.instrument_id != trade.intent.instrument_id {
        return false;
    }
    trade
        .snapshot
        .market_id
        .as_ref()
        .is_none_or(|market_id| settlement.market_id.as_ref() == Some(market_id))
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
