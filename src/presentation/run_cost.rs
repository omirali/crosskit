// What a run that asked a model kept and what it cost, in the lines `index jobs run` and every
// `index watch` pass print. One wording for both, so the two never disagree about a number.
use crate::application::CostRate;
use crate::domain::RunSpend;

/// How long one call of the knowledge question takes, measured on the owner's first run of it
/// (`work`, 2026-10-06: fifteen calls, about 27 s each). A long input takes longer; the number
/// is what a forecast plans with, and it says so.
pub(super) const SECONDS_PER_CALL: f64 = 27.0;

/// How large the calls `SECONDS_PER_CALL` was measured on were: 65 KB over fifteen calls,
/// about 4 KB each. A forecast for calls much larger than that is a lower bound and says so.
const MEASURED_BYTES_PER_CALL: i64 = 20_000;

/// What `calls` calls carrying `bytes` would cost at this context's rate, and roughly how long.
pub(super) fn forecast(rate: Option<CostRate>, calls: usize, bytes: i64) -> String {
    let mut time = duration(calls as f64 * SECONDS_PER_CALL);
    if calls > 0 && bytes / calls as i64 > MEASURED_BYTES_PER_CALL {
        time = format!("at least {time} (longer inputs than the time was measured on)");
    }
    match rate {
        Some(rate) => {
            let micro =
                rate.per_call_micro_usd * calls as f64 + rate.per_byte_micro_usd * bytes as f64;
            format!(
                "about {} and {time} at this context's rate ({} paid calls)",
                dollars(micro.round() as i64),
                rate.calls
            )
        }
        None => format!("{time}; no call has been paid in this context yet, so no price"),
    }
}

/// Seconds as a person plans with them: seconds below a minute and a half, minutes below an
/// hour, hours above it.
pub(super) fn duration(seconds: f64) -> String {
    match seconds {
        seconds if seconds < 90.0 => format!("~{seconds:.0} s"),
        seconds if seconds < 3600.0 => format!("~{:.0} min", seconds / 60.0),
        seconds => format!("~{:.1} h", seconds / 3600.0),
    }
}

/// Two lines: what of the answers was kept, and what the calls cost.
pub(super) fn spend_lines(spend: &RunSpend) -> [String; 2] {
    let tally = spend.tally();
    let proposed = tally.facts_proposed();
    [
        format!(
            "Knowledge: {} facts kept of {proposed} proposed — {} rejected because the quote is not in its part, {} because it does not name both ends, {} otherwise; {} entities, {} spellings glued",
            tally.facts_kept,
            tally.facts_rejected_by_quote,
            tally.facts_rejected_unnamed,
            tally.facts_rejected_otherwise,
            tally.entities_kept,
            tally.spellings_glued,
        ),
        format!(
            "Cost: {} calls ({} answered from the cache), {} bytes sent, {} input and {} output tokens, {} as the engine reported it",
            spend.calls(),
            spend.cache_hits(),
            spend.bytes(),
            spend.input_tokens(),
            spend.output_tokens(),
            dollars(spend.cost_micro_usd()),
        ),
    ]
}

/// Millionths of a dollar as a person reads them: `$0.0421`, or `no price` for zero —
/// the API engine reports tokens and no price.
pub(super) fn dollars(micro_usd: i64) -> String {
    if micro_usd <= 0 {
        return "no price".to_string();
    }
    format!("${:.4}", micro_usd as f64 / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::{dollars, duration, forecast, spend_lines};
    use crate::application::CostRate;
    use crate::domain::knowledge::KnowledgeTally;
    use crate::domain::{InsightProvenance, RunSpend};

    #[test]
    fn a_spend_is_said_in_two_lines_with_every_rejection_by_why() {
        let mut spend = RunSpend::default();
        spend.record_call(
            2_048,
            &InsightProvenance::from_model("m", "a-model", "p", 900, 120).costing(42_100),
        );
        spend.record_cache_hit();
        spend.record_knowledge(&KnowledgeTally {
            facts_kept: 7,
            facts_rejected_by_quote: 2,
            facts_rejected_unnamed: 3,
            facts_rejected_otherwise: 1,
            entities_kept: 6,
            spellings_glued: 1,
            ..KnowledgeTally::default()
        });

        let [knowledge, cost] = spend_lines(&spend);

        assert_eq!(
            knowledge,
            "Knowledge: 7 facts kept of 13 proposed — 2 rejected because the quote is not in its part, 3 because it does not name both ends, 1 otherwise; 6 entities, 1 spellings glued"
        );
        assert_eq!(
            cost,
            "Cost: 1 calls (1 answered from the cache), 2048 bytes sent, 900 input and 120 output tokens, $0.0421 as the engine reported it"
        );
        assert_eq!(dollars(0), "no price");
    }

    #[test]
    fn a_forecast_is_the_rate_times_the_calls_and_their_bytes() {
        let rate = CostRate {
            per_call_micro_usd: 50_000.0,
            per_byte_micro_usd: 10.0,
            calls: 15,
        };

        assert_eq!(
            forecast(Some(rate), 2, 100_000),
            "about $1.1000 and at least ~54 s (longer inputs than the time was measured on) at this context's rate (15 paid calls)"
        );
        assert_eq!(
            forecast(None, 200, 0),
            "~1.5 h; no call has been paid in this context yet, so no price"
        );
        assert_eq!(duration(600.0), "~10 min");
        assert_eq!(
            forecast(Some(rate), 2, 2_000),
            "about $0.1200 and ~54 s at this context's rate (15 paid calls)"
        );
    }
}
