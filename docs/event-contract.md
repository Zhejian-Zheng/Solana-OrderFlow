## 事件合同（Kafka Value：JSON）

### Topic

- `escrow.events.v1`
- `escrow.alerts.v1`（风控输出，可选）

### NormalizedEvent（escrow.events.v1）

字段（建议最小集合）：

- `event_id`: string（建议：`signature:instruction_index:log_index`）
- `event_type`: `"OfferCreated" | "OfferFilled" | "OfferCancelled"`
- `cluster`: `"localnet" | "devnet" | "mainnet-beta" | string`
- `slot`: number（u64）
- `signature`: string
- `program_id`: string
- `offer_id`: string（统一转 string，便于跨语言）
- `maker`: string（base58 pubkey）
- `taker`: string | null
- `mint_a`: string
- `mint_b`: string
- `amount_a`: string（u64 以 string 编码，避免 JS 精度问题）
- `amount_b`: string
- `commitment`: `"processed" | "confirmed" | "finalized"`
- `ts_ingest_ms`: number（unix ms）

示例：

```json
{
  "event_id": "5k...sig:0:12",
  "event_type": "OfferCreated",
  "cluster": "localnet",
  "slot": 123456,
  "signature": "5k...sig",
  "program_id": "EscroW...111",
  "offer_id": "42",
  "maker": "8g...maker",
  "taker": null,
  "mint_a": "So11111111111111111111111111111111111111112",
  "mint_b": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
  "amount_a": "1000000",
  "amount_b": "2000000",
  "commitment": "finalized",
  "ts_ingest_ms": 1730000000000
}
```

### AlertEvent（escrow.alerts.v1）

- `alert_id`: string（幂等键）
- `rule_id`: string
- `severity`: `"low" | "medium" | "high"`
- `maker`: string
- `offer_id`: string | null
- `ts_ms`: number
- `details`: object

