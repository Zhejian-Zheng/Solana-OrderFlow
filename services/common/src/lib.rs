use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedEvent {
    pub event_id: String,
    pub event_type: EventType,
    pub cluster: String,
    pub slot: u64,
    pub signature: String,
    pub program_id: String,
    pub offer_id: String,
    pub maker: String,
    pub taker: Option<String>,
    pub mint_a: String,
    pub mint_b: String,
    /// u64 encoded as string to avoid JS precision issues
    pub amount_a: String,
    /// u64 encoded as string to avoid JS precision issues
    pub amount_b: String,
    pub commitment: String,
    pub ts_ingest_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventType {
    OfferCreated,
    OfferFilled,
    OfferCancelled,
}

/// The on-chain JSON log payload (demo format).
/// This is *not* the Kafka contract; Kafka uses `NormalizedEvent`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnchainLogEvent {
    pub event: String,
    pub offer_id: String,
    pub maker: String,
    pub taker: Option<String>,
    pub mint_a: String,
    pub mint_b: String,
    pub amount_a: u64,
    pub amount_b: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEvent {
    pub alert_id: String,
    pub rule_id: String,
    pub severity: String,
    pub maker: String,
    pub offer_id: Option<String>,
    pub ts_ms: u64,
    pub details: serde_json::Value,
}

pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn parse_u64_str(s: &str) -> Option<u64> {
    s.parse::<u64>().ok()
}

