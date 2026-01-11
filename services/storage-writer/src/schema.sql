-- events: append-only, idempotency via event_id primary key
create table if not exists events (
  event_id text primary key,
  event_type text not null,
  signature text not null,
  slot bigint not null,
  offer_id text not null,
  payload_json jsonb not null,
  ingested_at timestamptz not null default now()
);

create index if not exists idx_events_offer_id on events (offer_id);
create index if not exists idx_events_slot on events (slot);

-- offers: latest snapshot (rebuildable from events)
create table if not exists offers (
  offer_id text primary key,
  status text not null,
  maker text not null,
  taker text,
  mint_a text not null,
  mint_b text not null,
  amount_a bigint not null,
  amount_b bigint not null,
  created_slot bigint,
  updated_slot bigint not null,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists idx_offers_maker on offers (maker);
create index if not exists idx_offers_updated_slot on offers (updated_slot);

