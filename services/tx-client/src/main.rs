use anyhow::{Context, Result};
use borsh::BorshSerialize;
use clap::Parser;
use sha2::{Digest, Sha256};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::native_token::LAMPORTS_PER_SOL;
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Keypair, Signer};
use solana_sdk::system_instruction;
use solana_sdk::transaction::Transaction;
use spl_associated_token_account::get_associated_token_address;

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, env = "SOLANA_RPC_URL", default_value = "http://127.0.0.1:8899")]
    rpc_url: String,

    #[arg(long, env = "PROGRAM_ID")]
    program_id: String,

    /// Wallet keypair path (will act as maker)
    #[arg(long, env = "MAKER_KEYPAIR", default_value = "/home/zhejian/.config/solana/id.json")]
    maker_keypair: String,

    /// Optional taker keypair; if not provided, a random keypair is generated and airdropped
    #[arg(long, env = "TAKER_KEYPAIR")]
    taker_keypair: Option<String>,

    #[arg(long, default_value_t = 42)]
    offer_id: u64,

    #[arg(long, default_value_t = 1_000)]
    amount_a: u64,

    #[arg(long, default_value_t = 2_000)]
    amount_b: u64,

    /// cancel | take
    #[arg(long, default_value = "cancel")]
    action: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let program_id: Pubkey = args.program_id.parse().context("parse program_id")?;

    let rpc = RpcClient::new_with_commitment(args.rpc_url.clone(), CommitmentConfig::confirmed());

    let maker = read_keypair(&args.maker_keypair).context("read maker keypair")?;
    let taker: Keypair = if let Some(p) = args.taker_keypair.as_ref() {
        read_keypair(p).context("read taker keypair")?
    } else {
        Keypair::new()
    };

    // Ensure taker has SOL (for fees + ATA creation). Maker usually already has SOL in localnet.
    maybe_airdrop(&rpc, &taker.pubkey(), 2 * LAMPORTS_PER_SOL).await?;

    // Create two mints (A/B) owned by maker (mint authority).
    let mint_a = create_mint(&rpc, &maker, 6).await?;
    let mint_b = create_mint(&rpc, &maker, 6).await?;

    // Create ATAs
    let maker_ata_a = get_associated_token_address(&maker.pubkey(), &mint_a);
    let maker_ata_b = get_associated_token_address(&maker.pubkey(), &mint_b);
    let taker_ata_a = get_associated_token_address(&taker.pubkey(), &mint_a);
    let taker_ata_b = get_associated_token_address(&taker.pubkey(), &mint_b);

    create_ata_if_missing(&rpc, &maker, &maker.pubkey(), &mint_a).await?;
    create_ata_if_missing(&rpc, &maker, &maker.pubkey(), &mint_b).await?;
    create_ata_if_missing(&rpc, &taker, &taker.pubkey(), &mint_a).await?;
    create_ata_if_missing(&rpc, &taker, &taker.pubkey(), &mint_b).await?;

    // Mint token A to maker, token B to taker
    mint_to(&rpc, &maker, &mint_a, &maker_ata_a, args.amount_a).await?;
    mint_to(&rpc, &maker, &mint_b, &taker_ata_b, args.amount_b).await?;

    // Derive escrow PDA + vault ATA (owner = escrow PDA)
    let (escrow_state, _bump) = Pubkey::find_program_address(
        &[
            b"escrow",
            maker.pubkey().as_ref(),
            &args.offer_id.to_le_bytes(),
        ],
        &program_id,
    );
    let vault_ata = get_associated_token_address(&escrow_state, &mint_a);

    // 1) create_offer (maker)
    let ix_create = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(maker.pubkey(), true),      // maker
            AccountMeta::new_readonly(mint_a, false),    // mint_a
            AccountMeta::new_readonly(mint_b, false),    // mint_b
            AccountMeta::new(escrow_state, false),       // escrow_state
            AccountMeta::new(vault_ata, false),          // vault_ata
            AccountMeta::new(maker_ata_a, false),        // maker_ata_a
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(spl_associated_token_account::id(), false),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
            AccountMeta::new_readonly(solana_sdk::sysvar::rent::id(), false),
        ],
        data: anchor_ix_data("create_offer", &(args.offer_id, args.amount_a, args.amount_b))?,
    };
    send_tx(&rpc, &[ix_create], &[&maker]).await?;
    eprintln!("sent create_offer offer_id={}", args.offer_id);

    if args.action == "take" {
        // maker ATA B is already created above; mint_b to maker not needed.
        let ix_take = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),     // taker
                AccountMeta::new_readonly(mint_a, false),   // mint_a
                AccountMeta::new_readonly(mint_b, false),   // mint_b
                AccountMeta::new(escrow_state, false),      // escrow_state
                AccountMeta::new(maker.pubkey(), false),    // maker (system account)
                AccountMeta::new(vault_ata, false),         // vault_ata
                AccountMeta::new(taker_ata_a, false),       // taker_ata_a
                AccountMeta::new(taker_ata_b, false),       // taker_ata_b
                AccountMeta::new(maker_ata_b, false),       // maker_ata_b
                AccountMeta::new_readonly(spl_token::id(), false),
                AccountMeta::new_readonly(spl_associated_token_account::id(), false),
            ],
            data: anchor_ix_data("take_offer", &())?,
        };
        send_tx(&rpc, &[ix_take], &[&taker]).await?;
        eprintln!("sent take_offer offer_id={}", args.offer_id);
    } else {
        let ix_cancel = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),     // maker
                AccountMeta::new_readonly(mint_a, false),   // mint_a
                AccountMeta::new(escrow_state, false),      // escrow_state
                AccountMeta::new(vault_ata, false),         // vault_ata
                AccountMeta::new(maker_ata_a, false),       // maker_ata_a
                AccountMeta::new_readonly(spl_token::id(), false),
                AccountMeta::new_readonly(spl_associated_token_account::id(), false),
            ],
            data: anchor_ix_data("cancel_offer", &())?,
        };
        send_tx(&rpc, &[ix_cancel], &[&maker]).await?;
        eprintln!("sent cancel_offer offer_id={}", args.offer_id);
    }

    Ok(())
}

fn read_keypair(path: &str) -> Result<Keypair> {
    read_keypair_file(path).map_err(|e| anyhow::anyhow!(e.to_string()))
}

fn anchor_discriminator(ix_name: &str) -> [u8; 8] {
    let preimage = format!("global:{ix_name}");
    let mut h = Sha256::new();
    h.update(preimage.as_bytes());
    let out = h.finalize();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&out[..8]);
    disc
}

fn anchor_ix_data<T: BorshSerialize>(ix_name: &str, args: &T) -> Result<Vec<u8>> {
    let mut data = Vec::with_capacity(8 + 32);
    data.extend_from_slice(&anchor_discriminator(ix_name));
    data.extend_from_slice(&args.try_to_vec()?);
    Ok(data)
}

async fn send_tx(rpc: &RpcClient, ixs: &[Instruction], signers: &[&dyn Signer]) -> Result<()> {
    let fee_payer = signers
        .first()
        .context("no signers")?
        .pubkey();
    let bh = rpc.get_latest_blockhash().await?;
    let tx = Transaction::new_signed_with_payer(ixs, Some(&fee_payer), signers, bh);
    let sig = rpc.send_and_confirm_transaction(&tx).await?;
    eprintln!("tx sig={sig}");
    Ok(())
}

async fn maybe_airdrop(rpc: &RpcClient, pubkey: &Pubkey, lamports: u64) -> Result<()> {
    // localnet: airdrop may fail if faucet is disabled; ignore if so.
    match rpc.request_airdrop(pubkey, lamports).await {
        Ok(sig) => {
            let _ = rpc.confirm_transaction(&sig).await;
        }
        Err(_) => {}
    }
    Ok(())
}

async fn create_mint(rpc: &RpcClient, payer_and_auth: &Keypair, decimals: u8) -> Result<Pubkey> {
    let mint = Keypair::new();
    let rent = rpc
        .get_minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN)
        .await?;
    let bh = rpc.get_latest_blockhash().await?;
    let create = system_instruction::create_account(
        &payer_and_auth.pubkey(),
        &mint.pubkey(),
        rent,
        spl_token::state::Mint::LEN as u64,
        &spl_token::id(),
    );
    let init = spl_token::instruction::initialize_mint2(
        &spl_token::id(),
        &mint.pubkey(),
        &payer_and_auth.pubkey(),
        None,
        decimals,
    )?;
    let tx = Transaction::new_signed_with_payer(
        &[create, init],
        Some(&payer_and_auth.pubkey()),
        &[payer_and_auth, &mint],
        bh,
    );
    rpc.send_and_confirm_transaction(&tx).await?;
    Ok(mint.pubkey())
}

async fn create_ata_if_missing(
    rpc: &RpcClient,
    payer: &Keypair,
    owner: &Pubkey,
    mint: &Pubkey,
) -> Result<()> {
    let ata = get_associated_token_address(owner, mint);
    if rpc.get_account(&ata).await.is_ok() {
        return Ok(());
    }
    let ix = spl_associated_token_account::instruction::create_associated_token_account(
        &payer.pubkey(),
        owner,
        mint,
        &spl_token::id(),
    );
    send_tx(rpc, &[ix], &[payer]).await?;
    Ok(())
}

async fn mint_to(
    rpc: &RpcClient,
    mint_authority: &Keypair,
    mint: &Pubkey,
    dest_ata: &Pubkey,
    amount: u64,
) -> Result<()> {
    let ix = spl_token::instruction::mint_to(
        &spl_token::id(),
        mint,
        dest_ata,
        &mint_authority.pubkey(),
        &[],
        amount,
    )?;
    send_tx(rpc, &[ix], &[mint_authority]).await?;
    Ok(())
}

