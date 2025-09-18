use anchor_lang::prelude::*;
use pyth_sdk_solana::{load_price_feed_from_account_info, PriceFeed as PythPriceFeed};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct PriceFeed {
    pub price: i64,
    pub conf: u64,
    pub expo: i32,
    pub timestamp: i64,
    pub next_update_time: i64,
}

impl PriceFeed {
    pub fn new_from_pyth(price_account_info: &AccountInfo) -> Result<Self> {
        let price_feed = load_price_feed_from_account_info(price_account_info)
            .map_err(|_| ErrorCode::InvalidPriceFeed)?;
        
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
            
        let price = price_feed.get_current_price()
            .ok_or(ErrorCode::StalePrice)?;
            
        // Ensure price is not too old (max 60 seconds)
        require!(
            current_time - price.publish_time < 60,
            ErrorCode::StalePrice
        );

        Ok(Self {
            price: price.price,
            conf: price.conf as u64,
            expo: price.expo,
            timestamp: current_time,
            next_update_time: current_time + 1, // Update every second
        })
    }

    pub fn get_adjusted_price(&self) -> Result<u64> {
        // Check if price needs update
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
            
        require!(
            current_time <= self.next_update_time,
            ErrorCode::StalePrice
        );

        // Handle negative prices
        if self.price < 0 {
            return Err(error!(ErrorCode::NegativePrice));
        }

        // Convert price to proper scale (handle exponent)
        let scaled_price = if self.expo < 0 {
            self.price as u64 / 10u64.pow(-self.expo as u32)
        } else {
            self.price as u64 * 10u64.pow(self.expo as u32)
        };

        // Apply confidence interval for safety (use 95% of price)
        let safe_price = scaled_price
            .checked_mul(95)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(100)
            .ok_or(ErrorCode::MathOverflow)?;

        Ok(safe_price)
    }
    
    pub fn validate_price_change(&self, old_price: u64, max_change_bps: u16) -> Result<()> {
        let new_price = self.get_adjusted_price()?;
        
        // Calculate price change in basis points
        let price_change_bps = if new_price > old_price {
            ((new_price - old_price) * 10000 / old_price) as u16
        } else {
            ((old_price - new_price) * 10000 / old_price) as u16
        };
        
        require!(
            price_change_bps <= max_change_bps,
            ErrorCode::ExcessivePriceChange
        );
        
        Ok(())
    }
}

#[error_code]
pub enum ErrorCode {
    #[msg("Invalid price feed account")]
    InvalidPriceFeed,
    #[msg("Price feed is stale")]
    StalePrice,
    #[msg("Negative price not supported")]
    NegativePrice,
    #[msg("Math overflow")]
    MathOverflow,
}
