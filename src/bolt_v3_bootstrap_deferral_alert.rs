use serde::{Deserialize, Serialize};

pub const H7_BOOTSTRAP_CONST_ALERT_OWNER: &str = "H7_ALERT_OWNER_UNASSIGNED";
pub const H7_BOOTSTRAP_CONST_TRACKING_ISSUE_URL: &str =
    "https://github.com/seungpyoson/bolt-v2/issues/1079";
pub const H7_BOOTSTRAP_CONST_HARD_DEADLINE_UNIX_SECS: u64 = 1_785_456_000;
pub const H7_BOOTSTRAP_CONST_PRE_EXPIRY_ALERT_WINDOW_SECS: u64 = 7 * 24 * 60 * 60;
pub const H7_BOOTSTRAP_CONST_ALERT_METRIC_NAME: &str =
    "bolt_v3_bootstrap_const_deferral_pre_expiry_alert_total";
const H7_BOOTSTRAP_CONST_ALERT_METRIC_VALUE: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoltV3BootstrapDeferralAlertEvidence {
    pub owner: String,
    pub tracking_issue_url: String,
    pub deadline_unix_secs: u64,
    pub seconds_until_deadline: u64,
    pub alert_window_secs: u64,
    pub metric_name: String,
    pub metric_value: u64,
}

pub fn h7_bootstrap_deferral_alert_evidence(
    now_unix_secs: u64,
) -> Option<BoltV3BootstrapDeferralAlertEvidence> {
    let seconds_until_deadline =
        H7_BOOTSTRAP_CONST_HARD_DEADLINE_UNIX_SECS.saturating_sub(now_unix_secs);
    if seconds_until_deadline > H7_BOOTSTRAP_CONST_PRE_EXPIRY_ALERT_WINDOW_SECS {
        return None;
    }
    Some(BoltV3BootstrapDeferralAlertEvidence {
        owner: H7_BOOTSTRAP_CONST_ALERT_OWNER.to_string(),
        tracking_issue_url: H7_BOOTSTRAP_CONST_TRACKING_ISSUE_URL.to_string(),
        deadline_unix_secs: H7_BOOTSTRAP_CONST_HARD_DEADLINE_UNIX_SECS,
        seconds_until_deadline,
        alert_window_secs: H7_BOOTSTRAP_CONST_PRE_EXPIRY_ALERT_WINDOW_SECS,
        metric_name: H7_BOOTSTRAP_CONST_ALERT_METRIC_NAME.to_string(),
        metric_value: H7_BOOTSTRAP_CONST_ALERT_METRIC_VALUE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_const_pre_expiry_alert_fires_at_window_boundary() {
        let now = H7_BOOTSTRAP_CONST_HARD_DEADLINE_UNIX_SECS
            - H7_BOOTSTRAP_CONST_PRE_EXPIRY_ALERT_WINDOW_SECS;

        let alert = h7_bootstrap_deferral_alert_evidence(now)
            .expect("alert must fire exactly at the pre-expiry window boundary");

        assert_eq!(
            alert.seconds_until_deadline,
            H7_BOOTSTRAP_CONST_PRE_EXPIRY_ALERT_WINDOW_SECS
        );
        assert_eq!(alert.owner, H7_BOOTSTRAP_CONST_ALERT_OWNER);
        assert_eq!(
            alert.tracking_issue_url,
            H7_BOOTSTRAP_CONST_TRACKING_ISSUE_URL
        );
        assert_eq!(alert.metric_name, H7_BOOTSTRAP_CONST_ALERT_METRIC_NAME);
        assert_eq!(alert.metric_value, H7_BOOTSTRAP_CONST_ALERT_METRIC_VALUE);
    }

    #[test]
    fn bootstrap_const_pre_expiry_alert_does_not_fire_before_window() {
        let now = H7_BOOTSTRAP_CONST_HARD_DEADLINE_UNIX_SECS
            - H7_BOOTSTRAP_CONST_PRE_EXPIRY_ALERT_WINDOW_SECS
            - 1;

        assert_eq!(h7_bootstrap_deferral_alert_evidence(now), None);
    }
}
