use anyhow::{Context, Result};
use clap::Parser;
use futures::StreamExt;
use orderflow_common::{EventType, NormalizedEvent};
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer, StreamConsumer};
use rdkafka::message::Message;
use tokio_postgres::NoTls;

#[derive(Debug, Parser)]
struct Args {
    /// Kafka brokers, e.g. localhost:9092
    #[arg(long, env = "KAFKA_BROKERS", default_value = "localhost:9092")]
    kafka_brokers: String,

    #[arg(long, env = "KAFKA_TOPIC", default_value = "escrow.events.v1")]
    kafka_topic: String,

    #[arg(long, env = "KAFKA_GROUP_ID", default_value = "storage-writer-v1")]
    kafka_group_id: String,

    /// Postgres connection string
    #[arg(
        long,
        env = "DATABASE_URL",
        default_value = "postgres://orderflow:orderflow@localhost:5432/orderflow"
    )]
    database_url: String,
}

const SCHEMA_SQL: &str = include_str!("schema.sql");

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let (db, conn) = tokio_postgres::connect(&args.database_url, NoTls)
        .await
        .context("connect postgres")?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            eprintln!("postgres connection error: {e:?}");
        }
    });

    db.batch_execute(SCHEMA_SQL)
        .await
        .context("apply schema")?;

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
        .subscribe(&[&args.kafka_topic])
        .context("subscribe topic")?;

    eprintln!(
        "storage-writer started: brokers={} topic={} group={} db={}",
        args.kafka_brokers, args.kafka_topic, args.kafka_group_id, args.database_url
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

                if let Err(e) = handle_event(&db, &ev).await {
                    // at-least-once: do not commit on failure -> will retry
                    eprintln!("handle_event failed: {e:?} event_id={}", ev.event_id);
                    continue;
                }

                let _ = consumer.commit_message(&msg, CommitMode::Async);
            }
        }
    }

    Ok(())
}

async fn handle_event(db: &tokio_postgres::Client, ev: &NormalizedEvent) -> Result<()> {
    // 1) insert into events (idempotent)
    db.execute(
        r#"
        insert into events (event_id, event_type, signature, slot, offer_id, payload_json)
        values ($1, $2, $3, $4, $5, $6::jsonb)
        on conflict (event_id) do nothing
        "#,
        &[
            &ev.event_id,
            &format!("{:?}", ev.event_type),
            &ev.signature,
            &(ev.slot as i64),
            &ev.offer_id,
            &serde_json::to_string(ev)?,
        ],
    )
    .await
    .context("insert events")?;

    // 2) upsert offers snapshot (monotonic by updated_slot)
    let (status, taker) = match ev.event_type {
        EventType::OfferCreated => ("created", None),
        EventType::OfferFilled => ("filled", ev.taker.clone()),
        EventType::OfferCancelled => ("cancelled", None),
    };

    let amount_a: i64 = ev
        .amount_a
        .parse::<u64>()
        .unwrap_or(0)
        .min(i64::MAX as u64) as i64;
    let amount_b: i64 = ev
        .amount_b
        .parse::<u64>()
        .unwrap_or(0)
        .min(i64::MAX as u64) as i64;

    db.execute(
        r#"
        insert into offers
          (offer_id, status, maker, taker, mint_a, mint_b, amount_a, amount_b, created_slot, updated_slot)
        values
          ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        on conflict (offer_id) do update set
          status = excluded.status,
          maker = excluded.maker,
          taker = excluded.taker,
          mint_a = excluded.mint_a,
          mint_b = excluded.mint_b,
          amount_a = excluded.amount_a,
          amount_b = excluded.amount_b,
          created_slot = coalesce(offers.created_slot, excluded.created_slot),
          updated_slot = excluded.updated_slot,
          updated_at = now()
        where offers.updated_slot <= excluded.updated_slot
        "#,
        &[
            &ev.offer_id,
            &status,
            &ev.maker,
            &taker,
            &ev.mint_a,
            &ev.mint_b,
            &amount_a,
            &amount_b,
            &(ev.slot as i64),
            &(ev.slot as i64),
        ],
    )
    .await
    .context("upsert offers")?;

    Ok(())
}

