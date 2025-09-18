use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount};
use std::collections::VecDeque;
mod price_feed;
use price_feed::PriceFeed;

declare_id!("MeMePrP1111111111111111111111111111111111");

#[program]
pub mod memeperp {
    use super::*;

    pub fn initialize_market(
        ctx: Context<InitializeMarket>,
        market_name: String,
        min_base_order_size: u64,
        tick_size: u64,
        initial_leverage_max: u8,
        liquidation_threshold: u16,  // in basis points (e.g., 9500 = 95%)
        maintenance_margin_fraction: u16,  // in basis points
        max_position_size: u64,
        funding_interval: i64,  // in seconds
    ) -> Result<()> {
        let market = &mut ctx.accounts.market;
        market.name = market_name;
        market.authority = ctx.accounts.authority.key();
        market.min_base_order_size = min_base_order_size;
        market.tick_size = tick_size;
        market.max_leverage = initial_leverage_max;
        market.liquidation_threshold = liquidation_threshold;
        market.maintenance_margin_fraction = maintenance_margin_fraction;
        market.long_positions = VecDeque::new();
        market.short_positions = VecDeque::new();
        market.is_initialized = true;
        market.total_fee_accrued = 0;
        market.max_position_size = max_position_size;
        market.funding_rate = 0;
        market.last_funding_time = Clock::get()?.unix_timestamp;
        market.funding_interval = funding_interval;
        Ok(())
    }

    pub fn update_funding_rate(ctx: Context<UpdateFunding>) -> Result<()> {
        let market = &mut ctx.accounts.market;
        let clock = Clock::get()?;
        let current_time = clock.unix_timestamp;
        
        // Check if it's time to update funding
        if current_time - market.last_funding_time < market.funding_interval {
            return Ok(());
        }

        // Calculate imbalance between longs and shorts
        let total_long_size: u64 = market.long_positions.iter()
            .map(|pos| pos.size)
            .sum();
        let total_short_size: u64 = market.short_positions.iter()
            .map(|pos| pos.size)
            .sum();

        // Calculate funding rate based on imbalance
        // Rate is in basis points (1/10000)
        let imbalance_ratio = if total_short_size == 0 {
            1.0
        } else {
            total_long_size as f64 / total_short_size as f64
        };

        // Funding rate calculation:
        // - If longs > shorts, longs pay shorts
        // - If shorts > longs, shorts pay longs
        // - Max rate is 0.1% per funding interval
        let new_funding_rate = ((imbalance_ratio - 1.0) * 10.0) as i64;
        market.funding_rate = new_funding_rate.max(-10).min(10); // Clamp to ±0.1%
        market.last_funding_time = current_time;

        // Apply funding to all positions
        for position in market.long_positions.iter_mut() {
            apply_funding_to_position(position, market.funding_rate, true)?;
        }
        for position in market.short_positions.iter_mut() {
            apply_funding_to_position(position, market.funding_rate, false)?;
        }

        Ok(())
    }

    pub fn place_order(
        ctx: Context<PlaceOrder>,
        side: Side,
        size: u64,
        price: u64,
        leverage: u8,
    ) -> Result<()> {
        let market = &mut ctx.accounts.market;
        let user = &mut ctx.accounts.user;

        // Get current price from pump.fun oracle
        let price_feed = PriceFeed::new_from_pyth(&ctx.accounts.price_feed)?;
        let current_price = price_feed.get_adjusted_price()?;

        // Validate order parameters
        require!(leverage <= market.max_leverage, ErrorCode::LeverageTooHigh);
        require!(size >= market.min_base_order_size, ErrorCode::OrderTooSmall);
        require!(size <= market.max_position_size, ErrorCode::OrderTooLarge);
        require!(price % market.tick_size == 0, ErrorCode::InvalidPrice);

        // Calculate total position size after this order
        let total_size = match side {
            Side::Long => market.long_positions.iter().map(|p| p.size).sum::<u64>(),
            Side::Short => market.short_positions.iter().map(|p| p.size).sum::<u64>(),
        };
        
        require!(
            total_size.checked_add(size).ok_or(ErrorCode::MathOverflow)? <= market.max_position_size,
            ErrorCode::ExceedsMaxPosition
        );

        // Calculate required margin
        let required_margin = calculate_required_margin(size, current_price, leverage);
        
        // Calculate and collect fees (0.1% fee)
        let fee = (size * current_price) / 1000;
        market.total_fee_accrued = market.total_fee_accrued.checked_add(fee)
            .ok_or(ErrorCode::MathOverflow)?;

        // Verify user has enough collateral (including fees)
        require!(
            ctx.accounts.user_token_account.amount >= required_margin.checked_add(fee)
                .ok_or(ErrorCode::MathOverflow)?,
            ErrorCode::InsufficientCollateral
        );

        // Transfer margin and fees
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                token::Transfer {
                    from: ctx.accounts.user_token_account.to_account_info(),
                    to: ctx.accounts.market_vault.to_account_info(),
                    authority: user.to_account_info(),
                },
            ),
            required_margin.checked_add(fee).unwrap(),
        )?;

        // Create new position
        let position = Position {
            owner: user.key(),
            side,
            size,
            entry_price: current_price,
            leverage,
            margin: required_margin,
            last_funding_timestamp: Clock::get()?.unix_timestamp,
            liquidation_price: calculate_liquidation_price(
                side,
                current_price,
                leverage,
                market.liquidation_threshold,
            )?,
        };

        // Add position to the appropriate queue
        match side {
            Side::Long => market.long_positions.push_back(position),
            Side::Short => market.short_positions.push_back(position),
        }

        Ok(())
    }

    pub fn liquidate_position(
        ctx: Context<LiquidatePosition>,
        position_index: u64,
        side: Side,
    ) -> Result<()> {
        let market = &mut ctx.accounts.market;
        let price_feed = PriceFeed::new_from_pyth(&ctx.accounts.price_feed)?;
        let current_price = price_feed.get_adjusted_price()?;

        // Find and remove the position
        let position = match side {
            Side::Long => {
                require!(position_index < market.long_positions.len() as u64, ErrorCode::InvalidPositionIndex);
                market.long_positions.remove(position_index as usize)
            }
            Side::Short => {
                require!(position_index < market.short_positions.len() as u64, ErrorCode::InvalidPositionIndex);
                market.short_positions.remove(position_index as usize)
            }
        }.ok_or(ErrorCode::PositionNotFound)?;

        // Check if position can be liquidated
        let can_liquidate = match side {
            Side::Long => current_price <= position.liquidation_price,
            Side::Short => current_price >= position.liquidation_price,
        };

        require!(can_liquidate, ErrorCode::CannotLiquidate);

        // Calculate PnL and remaining margin
        let pnl = calculate_pnl(
            side,
            position.size,
            position.entry_price,
            current_price,
            position.leverage,
        )?;

        // Transfer remaining margin (if any) back to user
        let remaining_margin = if pnl > 0 {
            position.margin.checked_add(pnl).ok_or(ErrorCode::MathOverflow)?
        } else {
            position.margin.checked_sub(pnl.abs() as u64).ok_or(ErrorCode::MathOverflow)?
        };

        if remaining_margin > 0 {
            token::transfer(
                CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    token::Transfer {
                        from: ctx.accounts.market_vault.to_account_info(),
                        to: ctx.accounts.user_token_account.to_account_info(),
                        authority: market.to_account_info(),
                    },
                ),
                remaining_margin,
            )?;
        }

        Ok(())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq)]
pub enum Side {
    Long,
    Short,
}

#[account]
pub struct Market {
    pub name: String,
    pub authority: Pubkey,
    pub min_base_order_size: u64,
    pub tick_size: u64,
    pub max_leverage: u8,
    pub liquidation_threshold: u16,
    pub maintenance_margin_fraction: u16,
    pub long_positions: VecDeque<Position>,
    pub short_positions: VecDeque<Position>,
    pub is_initialized: bool,
    pub total_fee_accrued: u64,
    pub max_position_size: u64,
    pub funding_rate: i64,
    pub last_funding_time: i64,
    pub funding_interval: i64,  // in seconds
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct Position {
    pub owner: Pubkey,
    pub side: Side,
    pub size: u64,
    pub entry_price: u64,
    pub leverage: u8,
    pub margin: u64,
    pub last_funding_timestamp: i64,
    pub liquidation_price: u64,
    pub realized_pnl: i64,
    pub unrealized_pnl: i64,
    pub last_update_price: u64,
    pub creation_time: i64,
    pub total_funding_paid: i64,
}

impl Position {
    pub fn new(
        owner: Pubkey,
        side: Side,
        size: u64,
        entry_price: u64,
        leverage: u8,
        margin: u64,
        liquidation_price: u64,
    ) -> Self {
        let current_time = Clock::get().unwrap().unix_timestamp;
        Self {
            owner,
            side,
            size,
            entry_price,
            leverage,
            margin,
            last_funding_timestamp: current_time,
            liquidation_price,
            realized_pnl: 0,
            unrealized_pnl: 0,
            last_update_price: entry_price,
            creation_time: current_time,
            total_funding_paid: 0,
        }
    }

    pub fn update_unrealized_pnl(&mut self, current_price: u64) -> Result<()> {
        self.unrealized_pnl = calculate_pnl(
            self.side,
            self.size,
            self.entry_price,
            current_price,
            self.leverage,
        )?;
        self.last_update_price = current_price;
        Ok(())
    }

    pub fn get_health_ratio(&self, current_price: u64) -> Result<u16> {
        let position_value = (self.size as u128)
            .checked_mul(current_price as u128)
            .ok_or(ErrorCode::MathOverflow)?;
            
        let margin_ratio = (self.margin as u128)
            .checked_mul(10000)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(position_value)
            .ok_or(ErrorCode::MathOverflow)?;
            
        Ok(margin_ratio as u16)
    }

    pub fn can_be_liquidated(&self, current_price: u64, maintenance_margin_ratio: u16) -> Result<bool> {
        let health_ratio = self.get_health_ratio(current_price)?;
        Ok(health_ratio < maintenance_margin_ratio)
    }
}

#[derive(Accounts)]
pub struct InitializeMarket<'info> {
    #[account(init, payer = authority, space = 8 + 32 + 32 + 8 + 8 + 1 + 2 + 2 + 8 + 8 + 1 + 8)]
    pub market: Account<'info, Market>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct PlaceOrder<'info> {
    #[account(mut)]
    pub market: Account<'info, Market>,
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(mut)]
    pub user_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub market_vault: Account<'info, TokenAccount>,
    /// CHECK: Price feed account is verified in the PriceFeed implementation
    pub price_feed: AccountInfo<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct LiquidatePosition<'info> {
    #[account(mut)]
    pub market: Account<'info, Market>,
    #[account(mut)]
    pub user_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub market_vault: Account<'info, TokenAccount>,
    /// CHECK: Price feed account is verified in the PriceFeed implementation
    pub price_feed: AccountInfo<'info>,
    pub token_program: Program<'info, Token>,
}

#[error_code]
pub enum ErrorCode {
    #[msg("Order size is too small")]
    OrderTooSmall,
    #[msg("Order size exceeds maximum allowed")]
    OrderTooLarge,
    #[msg("Invalid price")]
    InvalidPrice,
    #[msg("Leverage too high")]
    LeverageTooHigh,
    #[msg("Insufficient collateral")]
    InsufficientCollateral,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Invalid position index")]
    InvalidPositionIndex,
    #[msg("Position not found")]
    PositionNotFound,
    #[msg("Cannot liquidate position")]
    CannotLiquidate,
    #[msg("Total position size exceeds market maximum")]
    ExceedsMaxPosition,
    #[msg("Invalid funding rate")]
    InvalidFundingRate,
    #[msg("Price feed is stale")]
    StalePrice,
    #[msg("Negative price not supported")]
    NegativePrice,
    #[msg("Invalid price feed account")]
    InvalidPriceFeed,
    #[msg("Price change exceeds maximum allowed")]
    ExcessivePriceChange,
    #[msg("Position is already liquidated")]
    AlreadyLiquidated,
    #[msg("Invalid market state")]
    InvalidMarketState,
    #[msg("Unauthorized operation")]
    Unauthorized,
    #[msg("Market is paused")]
    MarketPaused,
    #[msg("Invalid fee calculation")]
    InvalidFee,
    #[msg("Position margin too low")]
    MarginTooLow,
}

// Helper functions
fn calculate_required_margin(size: u64, price: u64, leverage: u8) -> u64 {
    (size * price) / leverage as u64
}

fn calculate_liquidation_price(
    side: Side,
    entry_price: u64,
    leverage: u8,
    liquidation_threshold: u16,
) -> Result<u64> {
    let threshold = liquidation_threshold as f64 / 10000.0;
    let price = entry_price as f64;
    
    let liquidation_price = match side {
        Side::Long => {
            price * (1.0 - (1.0 - threshold) * leverage as f64)
        }
        Side::Short => {
            price * (1.0 + (1.0 - threshold) * leverage as f64)
        }
    };
    
    Ok(liquidation_price as u64)
}

fn calculate_pnl(
    side: Side,
    size: u64,
    entry_price: u64,
    current_price: u64,
    leverage: u8,
) -> Result<i64> {
    let pnl = match side {
        Side::Long => {
            ((current_price as i128 - entry_price as i128) * size as i128 * leverage as i128) / entry_price as i128
        }
        Side::Short => {
            ((entry_price as i128 - current_price as i128) * size as i128 * leverage as i128) / entry_price as i128
        }
    };
    
    Ok(pnl as i64)
}

fn apply_funding_to_position(
    position: &mut Position,
    funding_rate: i64,
    is_long: bool,
) -> Result<()> {
    let funding_amount = if is_long {
        -((position.size as i128 * position.entry_price as i128 * funding_rate as i128) / 10000) as i64
    } else {
        ((position.size as i128 * position.entry_price as i128 * funding_rate as i128) / 10000) as i64
    };

    position.margin = if funding_amount > 0 {
        position.margin.checked_add(funding_amount as u64)
            .ok_or(ErrorCode::MathOverflow)?
    } else {
        position.margin.checked_sub((-funding_amount) as u64)
            .ok_or(ErrorCode::MathOverflow)?
    };

    Ok(())
}

#[derive(Accounts)]
pub struct UpdateFunding<'info> {
    #[account(mut)]
    pub market: Account<'info, Market>,
}