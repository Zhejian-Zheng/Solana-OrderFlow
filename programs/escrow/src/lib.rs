use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::{self, CloseAccount, Mint, Token, TokenAccount, Transfer};

// NOTE: 占位的有效 Pubkey（32-byte 全 0）。本地部署后用 `anchor keys list` 生成的 program id 替换它。
declare_id!("11111111111111111111111111111111");

#[program]
pub mod escrow {
    use super::*;

    pub fn create_offer(ctx: Context<CreateOffer>, offer_id: u64, amount_a: u64, amount_b: u64) -> Result<()> {
        require!(amount_a > 0, EscrowError::InvalidAmount);
        require!(amount_b > 0, EscrowError::InvalidAmount);

        let st = &mut ctx.accounts.escrow_state;
        st.version = 1;
        st.status = EscrowStatus::Created as u8;
        st.offer_id = offer_id;
        st.maker = ctx.accounts.maker.key();
        st.taker = Pubkey::default();
        st.mint_a = ctx.accounts.mint_a.key();
        st.mint_b = ctx.accounts.mint_b.key();
        st.amount_a = amount_a;
        st.amount_b = amount_b;
        st.escrow_bump = ctx.bumps.escrow_state;
        st.created_slot = Clock::get()?.slot;
        st.filled_slot = 0;
        st.cancelled_slot = 0;

        // maker token A -> vault ATA
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.maker_ata_a.to_account_info(),
                    to: ctx.accounts.vault_ata.to_account_info(),
                    authority: ctx.accounts.maker.to_account_info(),
                },
            ),
            amount_a,
        )?;

        // demo: stable JSON log line for off-chain parsing
        msg!(
            r#"{{"event":"OfferCreated","offer_id":"{}","maker":"{}","mint_a":"{}","amount_a":{},"mint_b":"{}","amount_b":{} }}"#,
            offer_id,
            st.maker,
            st.mint_a,
            amount_a,
            st.mint_b,
            amount_b
        );

        Ok(())
    }

    pub fn take_offer(ctx: Context<TakeOffer>) -> Result<()> {
        let st = &mut ctx.accounts.escrow_state;
        require!(st.status == EscrowStatus::Created as u8, EscrowError::InvalidStatus);

        // basic mint sanity checks (also enforced by account constraints)
        require_keys_eq!(ctx.accounts.mint_a.key(), st.mint_a, EscrowError::InvalidMint);
        require_keys_eq!(ctx.accounts.mint_b.key(), st.mint_b, EscrowError::InvalidMint);

        // taker token B -> maker token B
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.taker_ata_b.to_account_info(),
                    to: ctx.accounts.maker_ata_b.to_account_info(),
                    authority: ctx.accounts.taker.to_account_info(),
                },
            ),
            st.amount_b,
        )?;

        // vault token A -> taker token A (PDA signs via seeds/bump)
        let signer_seeds: &[&[u8]] = &[
            b"escrow",
            st.maker.as_ref(),
            &st.offer_id.to_le_bytes(),
            &[st.escrow_bump],
        ];

        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.vault_ata.to_account_info(),
                    to: ctx.accounts.taker_ata_a.to_account_info(),
                    authority: ctx.accounts.escrow_state.to_account_info(),
                },
                &[signer_seeds],
            ),
            st.amount_a,
        )?;

        st.status = EscrowStatus::Filled as u8;
        st.taker = ctx.accounts.taker.key();
        st.filled_slot = Clock::get()?.slot;

        msg!(
            r#"{{"event":"OfferFilled","offer_id":"{}","maker":"{}","taker":"{}","mint_a":"{}","amount_a":{},"mint_b":"{}","amount_b":{} }}"#,
            st.offer_id,
            st.maker,
            st.taker,
            st.mint_a,
            st.amount_a,
            st.mint_b,
            st.amount_b
        );

        // optional: close vault ATA to maker (saves rent)
        token::close_account(CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            CloseAccount {
                account: ctx.accounts.vault_ata.to_account_info(),
                destination: ctx.accounts.maker.to_account_info(),
                authority: ctx.accounts.escrow_state.to_account_info(),
            },
            &[signer_seeds],
        ))?;

        Ok(())
    }

    pub fn cancel_offer(ctx: Context<CancelOffer>) -> Result<()> {
        let st = &mut ctx.accounts.escrow_state;
        require!(st.status == EscrowStatus::Created as u8, EscrowError::InvalidStatus);
        require_keys_eq!(ctx.accounts.maker.key(), st.maker, EscrowError::Unauthorized);

        let signer_seeds: &[&[u8]] = &[
            b"escrow",
            st.maker.as_ref(),
            &st.offer_id.to_le_bytes(),
            &[st.escrow_bump],
        ];

        // vault token A -> maker token A (PDA signs)
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.vault_ata.to_account_info(),
                    to: ctx.accounts.maker_ata_a.to_account_info(),
                    authority: ctx.accounts.escrow_state.to_account_info(),
                },
                &[signer_seeds],
            ),
            st.amount_a,
        )?;

        st.status = EscrowStatus::Cancelled as u8;
        st.cancelled_slot = Clock::get()?.slot;

        msg!(
            r#"{{"event":"OfferCancelled","offer_id":"{}","maker":"{}","mint_a":"{}","amount_a":{},"mint_b":"{}","amount_b":{} }}"#,
            st.offer_id,
            st.maker,
            st.mint_a,
            st.amount_a,
            st.mint_b,
            st.amount_b
        );

        token::close_account(CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            CloseAccount {
                account: ctx.accounts.vault_ata.to_account_info(),
                destination: ctx.accounts.maker.to_account_info(),
                authority: ctx.accounts.escrow_state.to_account_info(),
            },
            &[signer_seeds],
        ))?;

        Ok(())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum EscrowStatus {
    Created = 0,
    Filled = 1,
    Cancelled = 2,
}

#[account]
pub struct EscrowState {
    pub version: u8,
    pub status: u8,
    pub escrow_bump: u8,
    pub _pad: [u8; 5],

    pub offer_id: u64,
    pub maker: Pubkey,
    pub taker: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub amount_a: u64,
    pub amount_b: u64,
    pub created_slot: u64,
    pub filled_slot: u64,
    pub cancelled_slot: u64,
}

impl EscrowState {
    pub const SPACE: usize = 8 /*disc*/ + 1 + 1 + 1 + 5 + 8 + 32 + 32 + 32 + 32 + 8 + 8 + 8 + 8 + 8;
}

#[derive(Accounts)]
#[instruction(offer_id: u64)]
pub struct CreateOffer<'info> {
    #[account(mut)]
    pub maker: Signer<'info>,

    pub mint_a: Account<'info, Mint>,
    pub mint_b: Account<'info, Mint>,

    #[account(
        init,
        payer = maker,
        space = EscrowState::SPACE,
        seeds = [b"escrow", maker.key().as_ref(), &offer_id.to_le_bytes()],
        bump
    )]
    pub escrow_state: Account<'info, EscrowState>,

    #[account(
        init_if_needed,
        payer = maker,
        associated_token::mint = mint_a,
        associated_token::authority = escrow_state
    )]
    pub vault_ata: Account<'info, TokenAccount>,

    #[account(
        mut,
        associated_token::mint = mint_a,
        associated_token::authority = maker
    )]
    pub maker_ata_a: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct TakeOffer<'info> {
    #[account(mut)]
    pub taker: Signer<'info>,

    pub mint_a: Account<'info, Mint>,
    pub mint_b: Account<'info, Mint>,

    #[account(
        mut,
        seeds = [b"escrow", escrow_state.maker.as_ref(), &escrow_state.offer_id.to_le_bytes()],
        bump = escrow_state.escrow_bump
    )]
    pub escrow_state: Account<'info, EscrowState>,

    /// maker is used as token-b receiver + vault close destination
    #[account(mut, address = escrow_state.maker)]
    pub maker: SystemAccount<'info>,

    #[account(
        mut,
        associated_token::mint = mint_a,
        associated_token::authority = escrow_state
    )]
    pub vault_ata: Account<'info, TokenAccount>,

    #[account(
        mut,
        associated_token::mint = mint_a,
        associated_token::authority = taker
    )]
    pub taker_ata_a: Account<'info, TokenAccount>,

    #[account(
        mut,
        associated_token::mint = mint_b,
        associated_token::authority = taker
    )]
    pub taker_ata_b: Account<'info, TokenAccount>,

    #[account(
        mut,
        associated_token::mint = mint_b,
        associated_token::authority = maker
    )]
    pub maker_ata_b: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
}

#[derive(Accounts)]
pub struct CancelOffer<'info> {
    #[account(mut)]
    pub maker: Signer<'info>,

    pub mint_a: Account<'info, Mint>,

    #[account(
        mut,
        seeds = [b"escrow", escrow_state.maker.as_ref(), &escrow_state.offer_id.to_le_bytes()],
        bump = escrow_state.escrow_bump
    )]
    pub escrow_state: Account<'info, EscrowState>,

    #[account(
        mut,
        associated_token::mint = mint_a,
        associated_token::authority = escrow_state
    )]
    pub vault_ata: Account<'info, TokenAccount>,

    #[account(
        mut,
        associated_token::mint = mint_a,
        associated_token::authority = maker
    )]
    pub maker_ata_a: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
}

#[error_code]
pub enum EscrowError {
    #[msg("invalid amount")]
    InvalidAmount,
    #[msg("invalid status")]
    InvalidStatus,
    #[msg("unauthorized")]
    Unauthorized,
    #[msg("invalid mint")]
    InvalidMint,
}

