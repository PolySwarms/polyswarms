use anchor_lang::prelude::*;
use anchor_lang::system_program::{transfer, Transfer};

declare_id!("Polyswarms1111111111111111111111111111111111");

// ---------- Program ----------
#[program]
pub mod polyswarms {
    use super::*;

    pub fn create_market(
        ctx: Context<CreateMarket>,
        params: CreateParams,
    ) -> Result<()> {
        let m = &mut ctx.accounts.market;
        m.creator = ctx.accounts.creator.key();
        m.resolver = params.resolver;
        m.fee_bps = params.fee_bps;
        require!(m.fee_bps <= 1000, ErrCode::FeeTooHigh); // <=10%
        m.status = Status::Open;
        m.close_ts = params.close_ts;
        m.bump = *ctx.bumps.get("market").unwrap();
        m.yes_pool_bump = *ctx.bumps.get("yes_pool").unwrap();
        m.no_pool_bump  = *ctx.bumps.get("no_pool").unwrap();
        m.yes_total = 0;
        m.no_total  = 0;
        m.outcome = None;
        Ok(())
    }

    pub fn close(ctx: Context<AuthMarket>) -> Result<()> {
        let m = &mut ctx.accounts.market;
        require!(m.status == Status::Open, ErrCode::InvalidState);
        require!(Clock::get()?.unix_timestamp >= m.close_ts, ErrCode::TooEarly);
        m.status = Status::Closed;
        Ok(())
    }

    pub fn place_bet(ctx: Context<PlaceBet>, side: Side, amount: u64) -> Result<()> {
        let m = &mut ctx.accounts.market;
        require!(m.status == Status::Open, ErrCode::InvalidState);
        require!(Clock::get()?.unix_timestamp < m.close_ts, ErrCode::MarketClosed);
        require!(amount > 0, ErrCode::ZeroAmount);

        // escrow: user -> pool
        let from = ctx.accounts.user.to_account_info();
        let to   = ctx.accounts.pool.to_account_info();
        let sp   = ctx.accounts.system_program.to_account_info();
        transfer(CpiContext::new(sp, Transfer { from, to }), amount)?;

        // tally
        match side {
            Side::Yes => m.yes_total = m.yes_total.checked_add(amount).ok_or(ErrCode::Overflow)?,
            Side::No  => m.no_total  = m.no_total.checked_add(amount).ok_or(ErrCode::Overflow)?,
        }

        // upsert Bet PDA
        let b = &mut ctx.accounts.bet;
        if b.amount == 0 {
            b.user = ctx.accounts.user.key();
            b.market = m.key();
            b.side = side;
            b.claimed = false;
        } else {
            require!(b.side == side, ErrCode::WrongSide);
        }
        b.amount = b.amount.checked_add(amount).ok_or(ErrCode::Overflow)?;
        Ok(())
    }

    pub fn resolve(ctx: Context<Resolve>, outcome: Side) -> Result<()> {
        let m = &mut ctx.accounts.market;
        require!(m.status == Status::Closed, ErrCode::InvalidState);
        require!(ctx.accounts.resolver.key() == m.resolver, ErrCode::Unauthorized);
        m.status = Status::Resolved;
        m.outcome = Some(outcome);

        // fee calculation on total pot
        let yes_bal = ctx.accounts.yes_pool.to_account_info().lamports();
        let no_bal  = ctx.accounts.no_pool.to_account_info().lamports();
        let total_pot = yes_bal.checked_add(no_bal).ok_or(ErrCode::Overflow)?;
        let fee = (total_pot as u128)
            .checked_mul(m.fee_bps as u128).ok_or(ErrCode::Overflow)?
            .checked_div(10_000).ok_or(ErrCode::Overflow)? as u64;

        // Move loser -> winner; collect protocol fee from pot
        let (winner_pool, winner_seeds) = pool_ai_and_seeds(&ctx.accounts.market, outcome, m.yes_pool_bump, m.no_pool_bump, &ctx.accounts.yes_pool, &ctx.accounts.no_pool)?;
        let (loser_pool , loser_seeds ) = pool_ai_and_seeds(&ctx.accounts.market, outcome.opposite(), m.yes_pool_bump, m.no_pool_bump, &ctx.accounts.yes_pool, &ctx.accounts.no_pool)?;

        // 1) take fee
        let mut remaining_fee = fee;
        let sys = ctx.accounts.system_program.to_account_info();
        let fee_vault = ctx.accounts.fee_vault.to_account_info();
        let loser_info = loser_pool.clone();
        let winner_info = winner_pool.clone();

        let loser_bal = loser_info.lamports();
        let take_from_loser = remaining_fee.min(loser_bal);
        if take_from_loser > 0 {
            transfer_signed(
                &loser_info, &fee_vault, &sys, take_from_loser, &loser_seeds
            )?;
            remaining_fee -= take_from_loser;
        }
        if remaining_fee > 0 {
            // take rest from winner pool
            transfer_signed(
                &winner_info, &fee_vault, &sys, remaining_fee, &winner_seeds
            )?;
        }

        // 2) move all remaining loser funds to winner
        let loser_left = loser_pool.lamports();
        if loser_left > 0 {
            transfer_signed(
                &loser_pool.to_account_info(), &winner_pool.to_account_info(), &sys, loser_left, &loser_seeds
            )?;
        }

        Ok(())
    }

    pub fn claim(ctx: Context<Claim>) -> Result<()> {
        let m = &mut ctx.accounts.market;
        require!(m.status == Status::Resolved, ErrCode::InvalidState);
        let out = m.outcome.ok_or(ErrCode::InvalidState)?;
        let b = &mut ctx.accounts.bet;

        require!(!b.claimed, ErrCode::AlreadyClaimed);
        require!(b.market == m.key(), ErrCode::InvalidBet);
        require!(b.user == ctx.accounts.user.key(), ErrCode::Unauthorized);
        require!(b.side == out, ErrCode::LoserCannotClaim);

        let winners_total = match out { Side::Yes => m.yes_total, Side::No => m.no_total };
        let total_pot = m.yes_total.checked_add(m.no_total).ok_or(ErrCode::Overflow)?;
        let fee = (total_pot as u128)
            .checked_mul(m.fee_bps as u128).ok_or(ErrCode::Overflow)?
            .checked_div(10_000).ok_or(ErrCode::Overflow)? as u64;
        let distributable = total_pot.checked_sub(fee).ok_or(ErrCode::Overflow)?;
        require!(winners_total > 0, ErrCode::NothingToClaim);

        // payout = distributable * user_bet / winners_total
        let payout = ((distributable as u128)
            .checked_mul(b.amount as u128).ok_or(ErrCode::Overflow)?
            .checked_div(winners_total as u128).ok_or(ErrCode::Overflow)?) as u64;

        // transfer from winner pool -> user
        let (from_ai, seeds) = pool_ai_and_seeds(m, out, m.yes_pool_bump, m.no_pool_bump, &ctx.accounts.yes_pool, &ctx.accounts.no_pool)?;
        let sys = ctx.accounts.system_program.to_account_info();
        transfer_signed(&from_ai, &ctx.accounts.user.to_account_info(), &sys, payout, &seeds)?;

        b.claimed = true;
        Ok(())
    }
}

// ---------- Helpers ----------
fn pool_ai_and_seeds<'info>(
    market: &Account<'info, Market>,
    side: Side,
    yes_bump: u8,
    no_bump: u8,
    yes_pool: &'info AccountInfo<'info>,
    no_pool:  &'info AccountInfo<'info>,
) -> (AccountInfo<'info>, [&'info [u8]; 3]) {
    match side {
        Side::Yes => {
            (yes_pool.clone(), [b"pool", market.key().as_ref(), b"YES"])
        },
        Side::No => {
            (no_pool.clone(), [b"pool", market.key().as_ref(), b"NO"])
        }
    }
}

fn transfer_signed<'info>(
    from: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    sys: &AccountInfo<'info>,
    lamports: u64,
    seeds: &[&[u8]; 3],
) -> Result<()> {
    let bump = Pubkey::find_program_address(seeds, &crate::id()).1;
    let mut seeds_bump: Vec<&[u8]> = seeds.to_vec();
    let bump_slice: &[u8] = &[bump];
    seeds_bump.push(bump_slice);
    let ix = anchor_lang::solana_program::system_instruction::transfer(from.key, to.key, lamports);
    anchor_lang::solana_program::program::invoke_signed(
        &ix,
        &[from.clone(), to.clone(), sys.clone()],
        &[&seeds_bump],
    )?;
    Ok(())
}

// ---------- Accounts / State ----------
#[derive(Accounts)]
#[instruction(params: CreateParams)]
pub struct CreateMarket<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,

    /// CHECK: resolver can be any signer later
    pub resolver: UncheckedAccount<'info>,

    #[account(
      init,
      payer = creator,
      space = Market::SIZE,
      seeds = [b"market", creator.key().as_ref(), &params.market_id.to_le_bytes()],
      bump
    )]
    pub market: Account<'info, Market>,

    /// YES pool PDA holds SOL
    #[account(
      init,
      payer = creator,
      space = Pool::SIZE,
      seeds = [b"pool", market.key().as_ref(), b"YES"],
      bump
    )]
    pub yes_pool: Account<'info, Pool>,

    /// NO pool PDA holds SOL
    #[account(
      init,
      payer = creator,
      space = Pool::SIZE,
      seeds = [b"pool", market.key().as_ref(), b"NO"],
      bump
    )]
    pub no_pool: Account<'info, Pool>,

    /// Protocol fee vault PDA
    #[account(
      init_if_needed,
      payer = creator,
      space = 8,
      seeds = [b"fee_vault"],
      bump
    )]
    pub fee_vault: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct CreateParams {
    pub market_id: u64,
    pub resolver: Pubkey,
    pub fee_bps: u16,
    pub close_ts: i64,
}

#[derive(Accounts)]
pub struct AuthMarket<'info> {
    #[account(mut, has_one = creator)]
    pub market: Account<'info, Market>,
    pub creator: Signer<'info>,
}

#[derive(Accounts)]
pub struct PlaceBet<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub market: Account<'info, Market>,

    /// CHECK: PDA for selected side
    #[account(mut, seeds=[b"pool", market.key().as_ref(), side_seed(&side)], bump)]
    pub pool: AccountInfo<'info>,

    #[account(
      init_if_needed,
      payer = user,
      space = Bet::SIZE,
      seeds = [b"bet", market.key().as_ref(), user.key().as_ref(), side_seed(&side)],
      bump
    )]
    pub bet: Account<'info, Bet>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Resolve<'info> {
    #[account(mut)]
    pub resolver: Signer<'info>,

    #[account(mut)]
    pub market: Account<'info, Market>,

    /// YES
    #[account(mut, seeds=[b"pool", market.key().as_ref(), b"YES"], bump)]
    pub yes_pool: AccountInfo<'info>,

    /// NO
    #[account(mut, seeds=[b"pool", market.key().as_ref(), b"NO"], bump)]
    pub no_pool: AccountInfo<'info>,

    /// Protocol fee vault
    #[account(mut, seeds=[b"fee_vault"], bump)]
    pub fee_vault: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Claim<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub market: Account<'info, Market>,

    /// YES pool (kept for seeds; actual used depends on outcome)
    #[account(mut, seeds=[b"pool", market.key().as_ref(), b"YES"], bump)]
    pub yes_pool: AccountInfo<'info>,

    /// NO pool
    #[account(mut, seeds=[b"pool", market.key().as_ref(), b"NO"], bump)]
    pub no_pool: AccountInfo<'info>,

    #[account(mut, seeds=[b"bet", market.key().as_ref(), user.key().as_ref(), side_seed_enum(market.outcome.ok_or(ErrCode::InvalidState)?))], bump)]
    pub bet: Account<'info, Bet>,

    pub system_program: Program<'info, System>,
}

// ---------- Data ----------
#[account]
pub struct Market {
    pub creator: Pubkey,
    pub resolver: Pubkey,
    pub fee_bps: u16,
    pub status: Status,
    pub close_ts: i64,
    pub bump: u8,
    pub yes_pool_bump: u8,
    pub no_pool_bump: u8,
    pub yes_total: u64,
    pub no_total: u64,
    pub outcome: Option<Side>,
}
impl Market {
    pub const SIZE: usize = 8   // disc
        + 32 + 32 + 2 + 1 + 8 + 1 + 1 + 1 + 8 + 8 + 1 + 1; // rough padding for Option
}

#[account]
pub struct Pool {
    // marker; SOL lives in lamports; we keep a tiny account
}
impl Pool { pub const SIZE: usize = 8; }

#[account]
pub struct Bet {
    pub user: Pubkey,
    pub market: Pubkey,
    pub side: Side,
    pub amount: u64,
    pub claimed: bool,
}
impl Bet { pub const SIZE: usize = 8 + 32 + 32 + 1 + 8 + 1; }

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum Side { Yes, No }
impl Side {
    pub fn opposite(self) -> Side { if matches!(self, Side::Yes) { Side::No } else { Side::Yes } }
}
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum Status { Open, Closed, Resolved }

// ---------- Utils ----------
pub fn side_seed(s: &Side) -> &'static [u8] {
    match s { Side::Yes => b"YES", Side::No => b"NO" }
}
pub fn side_seed_enum(s: Side) -> &'static [u8] { side_seed(&s) }

// ---------- Errors ----------
#[error_code]
pub enum ErrCode {
    #[msg("Invalid state for this operation")]
    InvalidState,
    #[msg("Market not open")]
    MarketClosed,
    #[msg("Too early to close")]
    TooEarly,
    #[msg("Unauthorized")]
    Unauthorized,
    #[msg("Zero amount")]
    ZeroAmount,
    #[msg("Overflow")]
    Overflow,
    #[msg("Wrong bet side for account")]
    WrongSide,
    #[msg("Nothing to claim")]
    NothingToClaim,
    #[msg("Already claimed")]
    AlreadyClaimed,
    #[msg("Loser cannot claim")]
    LoserCannotClaim,
    #[msg("Invalid bet")]
    InvalidBet,
    #[msg("Fee too high")]
    FeeTooHigh,
}
