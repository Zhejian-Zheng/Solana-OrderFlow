## 总体架构与数据流

### 目标与边界

- **链上（Solana Program / Anchor）**：资产托管、原子交换、状态机与权限约束（最终真相源）。
- **链下（Listener + Kafka + 多消费者）**：把链上状态变化转为“可订阅事件流”，解耦实时业务（落库、风控、通知、看板）。
- **重要边界**：Kafka/DB/通知系统都不是“真相源”；真相永远来自链上状态。

### 事件流（从链上到链下）

1. 用户提交交易调用 Program：`create_offer` / `take_offer` / `cancel_offer`
2. Program 成功后输出结构化日志（demo 用 JSON `msg!()`；可升级为 Anchor `#[event]`）
3. Listener 通过 RPC WebSocket `logsSubscribe` 订阅 program 日志
4. Listener 解析日志，生成统一的 `NormalizedEvent`
5. Listener 写入 Kafka topic：`escrow.events.v1`
6. 多消费者分别处理：
   - `storage-writer`：落 Postgres（`events` append-only + `offers` 快照）
   - `risk-engine`：规则计算，输出 `escrow.alerts.v1`
   - `notifier`：控制台输出（可扩展 WebSocket/Telegram/email）

### 投递语义与幂等

- **投递语义**：Listener -> Kafka 使用 **至少一次（at-least-once）**。
- **幂等去重**：消费者按 `event_id` 去重（DB 侧 `events.event_id` unique/PK）。
- **分区 key**：建议 `key = offer_id`，确保同订单事件进入同分区，天然更顺序。

### Finality（Solana 特性）

- demo 默认订阅 `finalized`（更稳，延迟更高）
- 可通过配置切换 `confirmed`（更实时，需考虑回滚补偿）

