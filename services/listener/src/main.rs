use anyhow::{Context, Result};
use clap::Parser;
use futures::StreamExt;
use orderflow_common::{now_ms, EventType, NormalizedEvent, OnchainLogEvent};
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use solana_client::nonblocking::pubsub_client::PubsubClient;
use solana_client::rpc_config::RpcTransactionLogsConfig;
use solana_client::rpc_filter::RpcTransactionLogsFilter;
use solana_sdk::commitment_config::CommitmentConfig;

#[derive(Debug, Parser)]
struct Args {
    /// Solana WS endpoint, e.g. ws://127.0.0.1:8900
    #[arg(long, env = "SOLANA_WS_URL")]
    solana_ws_url: String,

    /// Program id to subscribe
    #[arg(long, env = "PROGRAM_ID")]
    program_id: String,

    /// localnet/devnet/mainnet-beta
    #[arg(long, env = "CLUSTER", default_value = "localnet")]
    cluster: String,

    /// processed/confirmed/finalized
    #[arg(long, env = "COMMITMENT", default_value = "finalized")]
    commitment: String,

    /// Kafka brokers, e.g. localhost:9092
    #[arg(long, env = "KAFKA_BROKERS", default_value = "localhost:9092")]
    kafka_brokers: String,

    #[arg(long, env = "KAFKA_TOPIC", default_value = "escrow.events.v1")]
    kafka_topic: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &args.kafka_brokers)
        .set("message.timeout.ms", "5000")
        .create()
        .context("create kafka producer")?;

    let commitment = match args.commitment.as_str() {
        "processed" => CommitmentConfig::processed(),
        "confirmed" => CommitmentConfig::confirmed(),
        _ => CommitmentConfig::finalized(),
    };

    let (mut client, mut stream) = PubsubClient::logs_subscribe(
        &args.solana_ws_url,
        RpcTransactionLogsFilter::Mentions(vec![args.program_id.clone()]),
        RpcTransactionLogsConfig {
            commitment: Some(commitment),
        },
    )
    .await
    .context("logs_subscribe")?;

    eprintln!(
        "listener started: program_id={} ws={} topic={} commitment={}",
        args.program_id, args.solana_ws_url, args.kafka_topic, args.commitment
    );

    // graceful shutdown on ctrl-c
    let mut shutdown = tokio::signal::ctrl_c();

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                eprintln!("shutdown requested");
                break;
            }
            maybe_msg = stream.next() => {
                let Some(resp) = maybe_msg else { break; };
                let value = resp.value;
                if value.err.is_some() {
                    continue;
                }

                let slot = resp.context.slot;
                let sig = value.signature.clone();
                for (log_index, line) in value.logs.iter().enumerate() {
                    // `msg!()` becomes: "Program log: <payload>"
                    const PREFIX: &str = "Program log: ";
                    let Some(json) = line.strip_prefix(PREFIX) else { continue; };
                    if !json.contains(r#""event":"#) {
                        continue;
                    }

                    let parsed: OnchainLogEvent = match serde_json::from_str(json) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let event_type = match parsed.event.as_str() {
                        "OfferCreated" => EventType::OfferCreated,
                        "OfferFilled" => EventType::OfferFilled,
                        "OfferCancelled" => EventType::OfferCancelled,
                        _ => continue,
                    };

                    // For demo we don't have instruction_index from logsSubscribe response.
                    let instruction_index = 0u32;
                    let event_id = format!("{}:{}:{}", sig, instruction_index, log_index);

                    let ev = NormalizedEvent {
                        event_id,
                        event_type,
                        cluster: args.cluster.clone(),
                        slot,
                        signature: sig.clone(),
                        program_id: args.program_id.clone(),
                        offer_id: parsed.offer_id.clone(),
                        maker: parsed.maker.clone(),
                        taker: parsed.taker.clone(),
                        mint_a: parsed.mint_a.clone(),
                        mint_b: parsed.mint_b.clone(),
                        amount_a: parsed.amount_a.to_string(),
                        amount_b: parsed.amount_b.to_string(),
                        commitment: args.commitment.clone(),
                        ts_ingest_ms: now_ms(),
                    };

                    let payload = serde_json::to_string(&ev).context("serialize event")?;

                    // key = offer_id, to keep same order per offer in Kafka partitioning
                    let record = FutureRecord::to(&args.kafka_topic)
                        .key(&ev.offer_id)
                        .payload(&payload);

                    // at-least-once: we don't de-dupe here; consumers handle idempotency via event_id
                    let _ = producer.send(record, std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    let _ = client.shutdown().await;
    Ok(())
}

