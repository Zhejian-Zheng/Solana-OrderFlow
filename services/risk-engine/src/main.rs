use anyhow::{Context, Result};
use clap::Parser;
use futures::StreamExt;
use orderflow_common::{now_ms, parse_u64_str, AlertEvent, EventType, NormalizedEvent};
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, env = "KAFKA_BROKERS", default_value = "localhost:9092")]
    kafka_brokers: String,

    #[arg(long, env = "EVENTS_TOPIC", default_value = "escrow.events.v1")]
    events_topic: String,

    #[arg(long, env = "ALERTS_TOPIC", default_value = "escrow.alerts.v1")]
    alerts_topic: String,

    #[arg(long, env = "KAFKA_GROUP_ID", default_value = "risk-engine-v1")]
    kafka_group_id: String,

    /// maker cancels >= N within window => alert
    #[arg(long, env = "CANCEL_THRESHOLD", default_value_t = 5)]
    cancel_threshold: usize,

    /// cancel window minutes
    #[arg(long, env = "CANCEL_WINDOW_MIN", default_value_t = 10)]
    cancel_window_min: u64,

    /// amount threshold (either amount_a or amount_b) => alert
    #[arg(long, env = "LARGE_AMOUNT_THRESHOLD", default_value_t = 1_000_000_000)]
    large_amount_threshold: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &args.kafka_brokers)
        .set("group.id", &args.kafka_group_id)
        .set("enable.partition.eof", "false")
        .set("session.timeout.ms", "6000")
        .set("enable.auto.commit", "false")
        .set("auto.offset.reset", "earliest")
        .create()
        .context("create kafka consumer")?;

    consumer
        .subscribe(&[&args.events_topic])
        .context("subscribe events")?;

    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &args.kafka_brokers)
        .set("message.timeout.ms", "5000")
        .create()
        .context("create kafka producer")?;

    eprintln!(
        "risk-engine started: brokers={} events_topic={} alerts_topic={} group={}",
        args.kafka_brokers, args.events_topic, args.alerts_topic, args.kafka_group_id
    );

    let mut cancels: HashMap<String, VecDeque<u64>> = HashMap::new();
    let mut emitted_alerts: HashSet<String> = HashSet::new(); // demo: in-mem de-dupe
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    let mut stream = consumer.stream();

    let window_ms = args.cancel_window_min * 60_000;

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                eprintln!("shutdown requested");
                break;
            }
            maybe = stream.next() => {
                let Some(msg) = maybe else { break; };
                let msg = match msg {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("kafka error: {e:?}");
                        continue;
                    }
                };
                let Some(payload) = msg.payload_view::<str>().and_then(|p| p.ok()) else {
                    let _ = consumer.commit_message(&msg, CommitMode::Async);
                    continue;
                };

                let ev: NormalizedEvent = match serde_json::from_str(payload) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("bad event json: {e:?} payload={payload}");
                        let _ = consumer.commit_message(&msg, CommitMode::Async);
                        continue;
                    }
                };

                // rule 1: large amount
                if let Some(alert) = large_amount_rule(&ev, args.large_amount_threshold) {
                    emit_alert(&producer, &args.alerts_topic, &mut emitted_alerts, alert).await?;
                }

                // rule 2: frequent cancel
                if ev.event_type == EventType::OfferCancelled {
                    let now = now_ms();
                    let q = cancels.entry(ev.maker.clone()).or_default();
                    q.push_back(now);
                    while let Some(front) = q.front().copied() {
                        if now.saturating_sub(front) > window_ms {
                            q.pop_front();
                        } else {
                            break;
                        }
                    }

                    if q.len() >= args.cancel_threshold {
                        let window_start = q.front().copied().unwrap_or(now);
                        let alert_id = format!("freq_cancel:{}:{}:{}", ev.maker, window_start, args.cancel_threshold);
                        let alert = AlertEvent {
                            alert_id,
                            rule_id: "freq_cancel".to_string(),
                            severity: "medium".to_string(),
                            maker: ev.maker.clone(),
                            offer_id: Some(ev.offer_id.clone()),
                            ts_ms: now,
                            details: json!({
                                "window_ms": window_ms,
                                "cancel_count": q.len(),
                                "threshold": args.cancel_threshold
                            }),
                        };
                        emit_alert(&producer, &args.alerts_topic, &mut emitted_alerts, alert).await?;
                    }
                }

                let _ = consumer.commit_message(&msg, CommitMode::Async);
            }
        }
    }

    Ok(())
}

fn large_amount_rule(ev: &NormalizedEvent, threshold: u64) -> Option<AlertEvent> {
    let a = parse_u64_str(&ev.amount_a).unwrap_or(0);
    let b = parse_u64_str(&ev.amount_b).unwrap_or(0);
    if a < threshold && b < threshold {
        return None;
    }
    Some(AlertEvent {
        alert_id: format!("large_amount:{}:{}:{}", ev.offer_id, ev.signature, ev.slot),
        rule_id: "large_amount".to_string(),
        severity: "high".to_string(),
        maker: ev.maker.clone(),
        offer_id: Some(ev.offer_id.clone()),
        ts_ms: now_ms(),
        details: json!({
            "amount_a": ev.amount_a,
            "amount_b": ev.amount_b,
            "threshold": threshold,
            "event_type": format!("{:?}", ev.event_type)
        }),
    })
}

async fn emit_alert(
    producer: &FutureProducer,
    alerts_topic: &str,
    emitted_alerts: &mut HashSet<String>,
    alert: AlertEvent,
) -> Result<()> {
    if !emitted_alerts.insert(alert.alert_id.clone()) {
        return Ok(());
    }
    let payload = serde_json::to_string(&alert).context("serialize alert")?;

    // key by maker for ordering
    let record = FutureRecord::to(alerts_topic)
        .key(&alert.maker)
        .payload(&payload);

    let _ = producer.send(record, std::time::Duration::from_secs(5)).await;
    eprintln!("ALERT: {} rule={}", alert.alert_id, alert.rule_id);
    Ok(())
}

