# Solana-OrderFlow
Event-driven Solana escrow with Rust/Anchor + Kafka: on-chain token custody, off-chain indexing, real-time risk/notification consumers, and PostgreSQL-backed dashboards.

## 这不是结束：当前 repo 提供了可运行 Demo 的完整链路代码

- **链上**：`programs/escrow`（Anchor escrow：`create_offer/take_offer/cancel_offer` + vault ATA 托管 + JSON 日志）
- **链下 Producer**：`services/listener`（Solana `logsSubscribe` -> 解析 JSON -> Kafka `escrow.events.v1`）
- **链下 Consumers**
  - `services/storage-writer`：Kafka -> Postgres（`events` 幂等表 + `offers` 快照表）
  - `services/risk-engine`：规则风控 -> Kafka `escrow.alerts.v1`
  - `services/notifier`：消费 events/alerts -> 控制台输出
- **基础设施**：`docker-compose.yml`（Redpanda(Kafka) + Postgres）
- **文档**：`docs/architecture.md`、`docs/event-contract.md`

## 本地运行（最小闭环）

### 0) 先起 Kafka + Postgres

```bash
cd /home/zhejian/Solana-OrderFlow
docker compose up -d
```

### 1) 安装链上依赖（需要你本机具备 Solana/Anchor/Rust）

本机当前缺少 `solana`/`anchor`/`cargo`，请先安装：

- Rust（建议 rustup）
- Solana CLI
- Anchor CLI

安装完成后：

```bash
anchor --version
solana --version
cargo --version
```

### 2) 启动本地 Solana + 部署 Program

```bash
anchor localnet
```

新开一个终端（或后台）：

```bash
anchor build
anchor deploy
anchor keys list
```

> 注意：本 repo 里 `Anchor.toml` 与 `programs/escrow/src/lib.rs` 的 program id 是占位符（有效 pubkey），部署后请用 `anchor keys list` 输出替换。

### 3) 启动 listener + consumers

建议使用 `finalized`：

```bash
export KAFKA_BROKERS=localhost:9092
export DATABASE_URL=postgres://orderflow:orderflow@localhost:5432/orderflow
export CLUSTER=localnet
export COMMITMENT=finalized
export SOLANA_WS_URL=ws://127.0.0.1:8900
export PROGRAM_ID=<替换为你的 program id>

# Producer
cargo run -p listener -- \
  --solana-ws-url "$SOLANA_WS_URL" \
  --program-id "$PROGRAM_ID"

# Consumers（分别在不同终端跑）
cargo run -p storage-writer
cargo run -p risk-engine
cargo run -p notifier
```

### 4) 触发链上交易（create/take/cancel）

你可以用 Anchor 的测试或自行写 TS client；demo 里链上事件会打印成 JSON 日志，listener 会转成 Kafka 事件。

## 事件与幂等

- Kafka topic：`escrow.events.v1`
- key：`offer_id`
- 至少一次投递；消费者通过 `events.event_id`（PK）做幂等去重。

