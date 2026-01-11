import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import BN from "bn.js";
import {
  ASSOCIATED_TOKEN_PROGRAM_ID,
  TOKEN_PROGRAM_ID,
  createMint,
  getAccount,
  getAssociatedTokenAddress,
  getOrCreateAssociatedTokenAccount,
  mintTo,
} from "@solana/spl-token";
import { expect } from "chai";

async function airdropIfNeeded(
  connection: anchor.web3.Connection,
  pubkey: PublicKey,
  lamports: number
) {
  const bal = await connection.getBalance(pubkey);
  if (bal >= lamports) return;
  const sig = await connection.requestAirdrop(pubkey, lamports);
  await connection.confirmTransaction(sig, "confirmed");
}

function u64LeBytes(n: BN): Buffer {
  const buf = Buffer.alloc(8);
  buf.writeBigUInt64LE(BigInt(n.toString()), 0);
  return buf;
}

async function expectThrows(p: Promise<unknown>) {
  let threw = false;
  try {
    await p;
  } catch {
    threw = true;
  }
  expect(threw).to.eq(true);
}

describe("escrow", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Escrow as Program;

  it("create_offer -> cancel_offer", async () => {
    const maker = provider.wallet as anchor.Wallet;
    const offerId = new BN(Date.now().toString()); // avoid collisions
    const amountA = new BN("1000");
    const amountB = new BN("2000");

    // Create two test mints
    const mintA = await createMint(
      provider.connection,
      maker.payer,
      maker.publicKey,
      null,
      0
    );
    const mintB = await createMint(
      provider.connection,
      maker.payer,
      maker.publicKey,
      null,
      0
    );

    // Maker ATAs
    const makerAtaA = (
      await getOrCreateAssociatedTokenAccount(
        provider.connection,
        maker.payer,
        mintA,
        maker.publicKey
      )
    ).address;
    await getOrCreateAssociatedTokenAccount(
      provider.connection,
      maker.payer,
      mintB,
      maker.publicKey
    );

    // Mint A to maker
    await mintTo(
      provider.connection,
      maker.payer,
      mintA,
      makerAtaA,
      maker.publicKey,
      BigInt(amountA.toString())
    );

    const [escrowState] = PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), maker.publicKey.toBuffer(), u64LeBytes(offerId)],
      program.programId
    );
    const vaultAta = await getAssociatedTokenAddress(mintA, escrowState, true);

    await program.methods
      .createOffer(offerId, amountA, amountB)
      .accounts({
        maker: maker.publicKey,
        mintA,
        mintB,
        escrowState,
        vaultAta,
        makerAtaA,
        tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    const vaultAfterCreate = await getAccount(provider.connection, vaultAta);
    expect(Number(vaultAfterCreate.amount)).to.eq(Number(amountA.toString()));

    await program.methods
      .cancelOffer()
      .accounts({
        maker: maker.publicKey,
        mintA,
        escrowState,
        vaultAta,
        makerAtaA,
        tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      })
      .rpc();

    // Vault should be closed
    let vaultClosed = false;
    try {
      await getAccount(provider.connection, vaultAta);
    } catch {
      vaultClosed = true;
    }
    expect(vaultClosed).to.eq(true);
  });

  it("create_offer -> take_offer (asset swap + vault close)", async () => {
    const maker = provider.wallet as anchor.Wallet;
    const taker = Keypair.generate();
    await airdropIfNeeded(provider.connection, taker.publicKey, 2e9); // ~2 SOL

    const offerId = new BN((Date.now() + 1).toString());
    const amountA = new BN("1000");
    const amountB = new BN("2000");

    // Create mints
    const mintA = await createMint(
      provider.connection,
      maker.payer,
      maker.publicKey,
      null,
      0
    );
    const mintB = await createMint(
      provider.connection,
      maker.payer,
      maker.publicKey,
      null,
      0
    );

    // ATAs
    const makerAtaA = (
      await getOrCreateAssociatedTokenAccount(
        provider.connection,
        maker.payer,
        mintA,
        maker.publicKey
      )
    ).address;
    const makerAtaB = (
      await getOrCreateAssociatedTokenAccount(
        provider.connection,
        maker.payer,
        mintB,
        maker.publicKey
      )
    ).address;
    const takerAtaA = (
      await getOrCreateAssociatedTokenAccount(
        provider.connection,
        taker,
        mintA,
        taker.publicKey
      )
    ).address;
    const takerAtaB = (
      await getOrCreateAssociatedTokenAccount(
        provider.connection,
        taker,
        mintB,
        taker.publicKey
      )
    ).address;

    // Mint A to maker, B to taker (maker is mint authority for both)
    await mintTo(
      provider.connection,
      maker.payer,
      mintA,
      makerAtaA,
      maker.publicKey,
      BigInt(amountA.toString())
    );
    await mintTo(
      provider.connection,
      maker.payer,
      mintB,
      takerAtaB,
      maker.publicKey,
      BigInt(amountB.toString())
    );

    const [escrowState] = PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), maker.publicKey.toBuffer(), u64LeBytes(offerId)],
      program.programId
    );
    const vaultAta = await getAssociatedTokenAddress(mintA, escrowState, true);

    await program.methods
      .createOffer(offerId, amountA, amountB)
      .accounts({
        maker: maker.publicKey,
        mintA,
        mintB,
        escrowState,
        vaultAta,
        makerAtaA,
        tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    // balances before
    const makerBAfterCreate = await getAccount(provider.connection, makerAtaB);
    const takerAAfterCreate = await getAccount(provider.connection, takerAtaA);
    expect(Number(makerBAfterCreate.amount)).to.eq(0);
    expect(Number(takerAAfterCreate.amount)).to.eq(0);

    await program.methods
      .takeOffer()
      .accounts({
        taker: taker.publicKey,
        mintA,
        mintB,
        escrowState,
        maker: maker.publicKey,
        vaultAta,
        takerAtaA,
        takerAtaB,
        makerAtaB,
        tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      })
      .signers([taker])
      .rpc();

    const makerBAfter = await getAccount(provider.connection, makerAtaB);
    const takerAAfter = await getAccount(provider.connection, takerAtaA);
    expect(Number(makerBAfter.amount)).to.eq(Number(amountB.toString()));
    expect(Number(takerAAfter.amount)).to.eq(Number(amountA.toString()));

    // Vault should be closed after take_offer
    let vaultClosed = false;
    try {
      await getAccount(provider.connection, vaultAta);
    } catch {
      vaultClosed = true;
    }
    expect(vaultClosed).to.eq(true);
  });

  it("cancel_offer should fail if not maker", async () => {
    const maker = provider.wallet as anchor.Wallet;
    const other = Keypair.generate();
    await airdropIfNeeded(provider.connection, other.publicKey, 2e9);

    const offerId = new BN((Date.now() + 2).toString());
    const amountA = new BN("1000");
    const amountB = new BN("2000");

    const mintA = await createMint(
      provider.connection,
      maker.payer,
      maker.publicKey,
      null,
      0
    );
    const mintB = await createMint(
      provider.connection,
      maker.payer,
      maker.publicKey,
      null,
      0
    );

    const makerAtaA = (
      await getOrCreateAssociatedTokenAccount(
        provider.connection,
        maker.payer,
        mintA,
        maker.publicKey
      )
    ).address;
    await mintTo(
      provider.connection,
      maker.payer,
      mintA,
      makerAtaA,
      maker.publicKey,
      BigInt(amountA.toString())
    );

    const [escrowState] = PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), maker.publicKey.toBuffer(), u64LeBytes(offerId)],
      program.programId
    );
    const vaultAta = await getAssociatedTokenAddress(mintA, escrowState, true);

    await program.methods
      .createOffer(offerId, amountA, amountB)
      .accounts({
        maker: maker.publicKey,
        mintA,
        mintB,
        escrowState,
        vaultAta,
        makerAtaA,
        tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    await expectThrows(
      program.methods
        .cancelOffer()
        .accounts({
          maker: other.publicKey,
          mintA,
          escrowState,
          vaultAta,
          makerAtaA,
          tokenProgram: TOKEN_PROGRAM_ID,
          associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        })
        .signers([other])
        .rpc()
    );
  });
});

