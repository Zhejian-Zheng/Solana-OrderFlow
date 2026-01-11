use anyhow::{Context, Result};
use clap::Parser;
use futures::StreamExt;
use orderflow_common::{AlertEvent, NormalizedEvent};
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer, StreamConsumer};
use rdkafka::message::Message;

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, env = "KAFKA_BROKERS", default_value = "localhost:9092")]
    kafka_brokers: String,

    #[arg(long, env = "EVENTS_TOPIC", default_value = "escrow.events.v1")]
    events_topic: String,

    #[arg(long, env = "ALERTS_TOPIC", default_value = "escrow.alerts.v1")]
    alerts_topic: String,

    #[arg(long, env = "KAFKA_GROUP_ID", default_value = "notifier-v1")]
    kafka_group_id: String,
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
        .subscribe(&[&args.events_topic, &args.alerts_topic])
        .context("subscribe topics")?;

    eprintln!(
        "notifier started: brokers={} events_topic={} alerts_topic={} group={}",
        args.kafka_brokers, args.events_topic, args.alerts_topic, args.kafka_group_id
    );

    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    let mut stream = consumer.stream();

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

                let topic = msg.topic().to_string();
                let Some(payload) = msg.payload_view::<str>().and_then(|p| p.ok()) else {
                    let _ = consumer.commit_message(&msg, CommitMode::Async);
                    continue;
                };

                if topic == args.events_topic {
                    if let Ok(ev) = serde_json::from_str::<NormalizedEvent>(payload) {
                        eprintln!(
                            "EVENT {:?}: offer_id={} maker={} taker={:?} a={} b={} slot={}",
                            ev.event_type, ev.offer_id, ev.maker, ev.taker, ev.amount_a, ev.amount_b, ev.slot
                        );
                    }
                } else if topic == args.alerts_topic {
                    if let Ok(al) = serde_json::from_str::<AlertEvent>(payload) {
                        eprintln!(
                            "ALERT severity={} rule={} maker={} offer_id={:?} alert_id={}",
                            al.severity, al.rule_id, al.maker, al.offer_id, al.alert_id
                        );
                    }
                }

                let _ = consumer.commit_message(&msg, CommitMode::Async);
            }
        }
    }

    Ok(())
}

