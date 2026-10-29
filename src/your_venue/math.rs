//! Off-chain port of `ntbundle`'s `instant_mint_bundle_tokens` math.
//!
//! Every function mirrors the on-chain rounding exactly (see
//! `bundle-sc/programs/ntbundle/src/instructions/instant_mint_bundle_tokens.rs`
//! and `charge_bundle_share_holder_fees_at_time` in `state/bundle.rs`) so the
//! quote matches simulated execution to the atom.

use crate::trading_venue::error::TradingVenueError;

use super::state::{Bundle, TokenizedBundleDepositor};

pub const BPS_DENOMINATOR: u128 = 10_000;
pub const SECONDS_IN_YEAR: u128 = 31_536_000;
/// Non-redeemable virtual units included in token/share conversions.
pub const SHARE_TOKEN_VIRTUAL_OFFSET: u128 = 1_000;

fn overflow() -> TradingVenueError {
    TradingVenueError::CheckedMathError("ntbundle math overflow".into())
}

pub fn checked_ceil_div(numerator: u128, denominator: u128) -> Result<u128, TradingVenueError> {
    let quotient = numerator.checked_div(denominator).ok_or_else(overflow)?;
    if numerator.is_multiple_of(denominator) {
        Ok(quotient)
    } else {
        quotient.checked_add(1).ok_or_else(overflow)
    }
}

pub fn fee_amount_ceil(amount: u64, fee_bps: u32) -> Result<u64, TradingVenueError> {
    let fee = checked_ceil_div(
        u128::from(amount)
            .checked_mul(u128::from(fee_bps))
            .ok_or_else(overflow)?,
        BPS_DENOMINATOR,
    )?;
    u64::try_from(fee).map_err(|_| overflow())
}

/// Floor share price, as charged to the share-token pool.
fn share_price(total_assets: u64, total_shares: u128, asset_precision: u64) -> Option<u128> {
    if total_shares == 0 {
        return Some(asset_precision as u128);
    }
    (total_assets as u128)
        .checked_mul(asset_precision as u128)?
        .checked_div(total_shares)
}

fn pool_profit(
    shares: u128,
    net_deposits: i128,
    cumulative_taxed_profit: u128,
    share_price: u128,
    asset_precision: u128,
) -> Option<i128> {
    let pool_value = shares
        .checked_mul(share_price)?
        .checked_div(asset_precision)?;
    let watermark = net_deposits.checked_add(i128::try_from(cumulative_taxed_profit).ok()?)?;
    i128::try_from(pool_value).ok()?.checked_sub(watermark)
}

/// Pool shares left after the fee crank `instant_mint_bundle_tokens` runs at
/// `current_time` before crediting the mint.
///
/// The crank moves management and performance fee shares from the pool to the
/// manager; bundle `total_shares` and total assets are unchanged, so only the
/// pool's share balance feeds back into the token conversion.
pub fn pool_shares_after_fees(
    bundle: &Bundle,
    pool: &TokenizedBundleDepositor,
    total_assets: u64,
    token_supply: u64,
    current_time: i64,
) -> Result<u128, TradingVenueError> {
    let mut net_deposits = pool.net_deposits;
    let mut cumulative_taxed_profit = pool.cumulative_taxed_profit;
    let mut remainder = pool.management_fee_remainder;
    if token_supply == 0 && pool.shares == 0 {
        net_deposits = 0;
        cumulative_taxed_profit = 0;
        remainder = 0;
    }

    let asset_precision = bundle.asset_precision as u128;
    let share_price = share_price(total_assets, bundle.total_shares, bundle.asset_precision)
        .ok_or_else(overflow)?;
    let holder_shares = pool.shares;

    if pool.last_management_fee_timestamp <= bundle.last_management_fee_change_timestamp {
        remainder = 0;
    }
    let mut management_fee_shares = 0u128;
    if holder_shares != 0 && pool.last_management_fee_timestamp > 0 {
        let accrual_start = pool
            .last_management_fee_timestamp
            .max(bundle.last_management_fee_change_timestamp);
        let seconds_elapsed = current_time
            .checked_sub(accrual_start)
            .ok_or_else(overflow)?
            .max(0) as u128;
        if seconds_elapsed > 0 {
            let numerator = holder_shares
                .checked_mul(seconds_elapsed)
                .and_then(|v| v.checked_mul(bundle.management_fee_bps as u128))
                .and_then(|v| v.checked_add(remainder))
                .ok_or_else(overflow)?;
            management_fee_shares = numerator / (SECONDS_IN_YEAR * BPS_DENOMINATOR);
        }
    }

    // `prepare_performance_fee_basis`: a zero or changed rate settles open gain
    // into the watermark without charging it.
    if bundle.performance_fee == 0 || pool.performance_fee_version != bundle.performance_fee_version
    {
        let profit = pool_profit(
            holder_shares,
            net_deposits,
            cumulative_taxed_profit,
            share_price,
            asset_precision,
        )
        .ok_or_else(overflow)?;
        if profit > 0 {
            cumulative_taxed_profit = cumulative_taxed_profit
                .checked_add(profit as u128)
                .ok_or_else(overflow)?;
        }
    }

    let profit = pool_profit(
        holder_shares,
        net_deposits,
        cumulative_taxed_profit,
        share_price,
        asset_precision,
    )
    .ok_or_else(overflow)?;
    let performance_fee_shares = if profit <= 0 || share_price == 0 {
        0
    } else {
        ((profit as u128)
            .checked_mul(bundle.performance_fee as u128)
            .ok_or_else(overflow)?
            / BPS_DENOMINATOR)
            .checked_mul(asset_precision)
            .ok_or_else(overflow)?
            / share_price
    };

    let minimum_remaining_shares = if token_supply == 0 { 0 } else { 1 };
    let fee_share_limit = holder_shares.saturating_sub(minimum_remaining_shares);
    let management_fee_shares = management_fee_shares.min(fee_share_limit);
    let performance_fee_shares =
        performance_fee_shares.min(fee_share_limit - management_fee_shares);

    Ok(holder_shares - management_fee_shares - performance_fee_shares)
}

/// Why an instant mint of a given size cannot execute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MintRejection {
    /// Below the bundle minimum, or rounds to zero shares/tokens.
    TooSmall,
    /// Exceeds the bundle deposit cap or the per-cycle instant-mint cap.
    CapacityExceeded,
}

/// Inputs to the per-size mint quote, precomputed at state update.
#[derive(Clone, Copy, Debug, Default)]
pub struct MintCurve {
    pub deposit_fee_bps: u32,
    pub issuance_fee_bps: u32,
    pub min_deposit_amount: u64,
    pub total_assets: u64,
    pub total_shares: u128,
    /// `total_assets + cumulative_pending_deposits`, checked against the cap.
    pub committed_assets: u64,
    pub max_deposit_amount: u64,
    /// Instant-mint volume already used this netting cycle.
    pub cycle_minted: u64,
    /// Zero means unlimited.
    pub mint_cap_per_cycle: u64,
    pub token_supply: u64,
    /// Pool shares after the fee crank.
    pub pool_shares: u128,
}

impl MintCurve {
    /// Receipt tokens minted for `amount_in` asset atoms, or why the mint
    /// would revert. Allocation-free.
    pub fn tokens_out(
        &self,
        amount_in: u64,
    ) -> Result<Result<u64, MintRejection>, TradingVenueError> {
        if amount_in < self.min_deposit_amount {
            return Ok(Err(MintRejection::TooSmall));
        }
        let deposit_fee = fee_amount_ceil(amount_in, self.deposit_fee_bps)?;
        let net_amount = amount_in - deposit_fee;

        let Some(committed) = self.committed_assets.checked_add(net_amount) else {
            return Ok(Err(MintRejection::CapacityExceeded));
        };
        if committed > self.max_deposit_amount {
            return Ok(Err(MintRejection::CapacityExceeded));
        }
        let Some(cycle_minted) = self.cycle_minted.checked_add(net_amount) else {
            return Ok(Err(MintRejection::CapacityExceeded));
        };
        if self.mint_cap_per_cycle != 0 && cycle_minted > self.mint_cap_per_cycle {
            return Ok(Err(MintRejection::CapacityExceeded));
        }

        let issuance_fee = fee_amount_ceil(net_amount, self.issuance_fee_bps)?;
        let net_after_issuance = net_amount - issuance_fee;
        let shares_minted = if self.total_assets == 0 || self.total_shares == 0 {
            u128::from(net_after_issuance)
        } else {
            u128::from(net_after_issuance)
                .checked_mul(self.total_shares)
                .ok_or_else(overflow)?
                / u128::from(self.total_assets)
        };
        if shares_minted == 0 {
            return Ok(Err(MintRejection::TooSmall));
        }

        let tokens = if self.token_supply == 0 {
            shares_minted
        } else {
            shares_minted
                .checked_mul(u128::from(self.token_supply) + SHARE_TOKEN_VIRTUAL_OFFSET)
                .ok_or_else(overflow)?
                / (self.pool_shares + SHARE_TOKEN_VIRTUAL_OFFSET)
        };
        if tokens == 0 {
            return Ok(Err(MintRejection::TooSmall));
        }
        Ok(Ok(u64::try_from(tokens).map_err(|_| overflow())?))
    }

    /// Marginal receipt tokens per asset atom. The mint is linear in
    /// `amount_in` up to integer rounding, so the derivative is the constant
    /// product of the fee haircuts and the share and token exchange rates.
    pub fn price(&self) -> f64 {
        let keep = (1.0 - self.deposit_fee_bps as f64 / BPS_DENOMINATOR as f64)
            * (1.0 - self.issuance_fee_bps as f64 / BPS_DENOMINATOR as f64);
        let shares_per_asset = if self.total_assets == 0 || self.total_shares == 0 {
            1.0
        } else {
            self.total_shares as f64 / self.total_assets as f64
        };
        let tokens_per_share = if self.token_supply == 0 {
            1.0
        } else {
            (self.token_supply as f64 + SHARE_TOKEN_VIRTUAL_OFFSET as f64)
                / (self.pool_shares as f64 + SHARE_TOKEN_VIRTUAL_OFFSET as f64)
        };
        keep * shares_per_asset * tokens_per_share
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve() -> MintCurve {
        MintCurve {
            deposit_fee_bps: 10,
            issuance_fee_bps: 5,
            min_deposit_amount: 1_000,
            total_assets: 2_000_000_000,
            total_shares: 1_900_000_000,
            committed_assets: 2_000_000_000,
            max_deposit_amount: 3_000_000_000,
            cycle_minted: 0,
            mint_cap_per_cycle: 0,
            token_supply: 500_000_000,
            pool_shares: 480_000_000,
        }
    }

    #[test]
    fn matches_on_chain_vectors() {
        // Mirrors `virtual_offset_vectors_match_v1` and the fee rounding tests
        // in instant_mint_bundle_tokens.rs.
        let c = MintCurve {
            token_supply: 500,
            pool_shares: 1_000,
            total_assets: 0,
            total_shares: 0,
            committed_assets: 0,
            max_deposit_amount: u64::MAX,
            min_deposit_amount: 0,
            ..Default::default()
        };
        assert_eq!(c.tokens_out(250).unwrap(), Ok(187));
        assert_eq!(fee_amount_ceil(101, 333).unwrap(), 4);
    }

    #[test]
    fn rejects_below_minimum_and_above_caps() {
        let c = curve();
        assert_eq!(c.tokens_out(999).unwrap(), Err(MintRejection::TooSmall));
        assert!(c.tokens_out(1_000).unwrap().is_ok());
        assert_eq!(
            c.tokens_out(1_100_000_000).unwrap(),
            Err(MintRejection::CapacityExceeded)
        );
        let capped = MintCurve {
            mint_cap_per_cycle: 10_000,
            cycle_minted: 9_000,
            ..curve()
        };
        assert_eq!(
            capped.tokens_out(2_000).unwrap(),
            Err(MintRejection::CapacityExceeded)
        );
    }

    #[test]
    fn price_tracks_finite_difference() {
        let c = curve();
        let f = |x: u64| c.tokens_out(x).unwrap().unwrap() as f64;
        for x in [100_000u64, 10_000_000, 500_000_000] {
            let h = x / 10;
            let fd = (f(x + h) - f(x - h)) / (2.0 * h as f64);
            assert!((fd - c.price()).abs() / c.price() < 1e-4, "x={x} fd={fd}");
        }
    }

    #[test]
    fn management_fee_crank_moves_shares_out_of_the_pool() {
        let bundle = Bundle {
            management_fee_bps: 200,
            performance_fee: 0,
            asset_precision: 1_000_000,
            total_shares: 1_000_000_000,
            ..Default::default()
        };
        let pool = TokenizedBundleDepositor {
            shares: SECONDS_IN_YEAR * 1_000,
            last_management_fee_timestamp: 100,
            ..Default::default()
        };
        let after =
            pool_shares_after_fees(&bundle, &pool, 1_000_000_000, 1, 100 + 365 * 86_400).unwrap();
        // 2% of a year's shares.
        assert_eq!(pool.shares - after, pool.shares * 2 / 100);
        // No elapsed time, no fee.
        assert_eq!(
            pool_shares_after_fees(&bundle, &pool, 1_000_000_000, 1, 100).unwrap(),
            pool.shares
        );
    }
}
