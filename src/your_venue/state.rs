//! Defensive, allocation-free readers for the `ntbundle` accounts the venue
//! quotes from.
//!
//! Layouts follow `bundle-sc/programs/ntbundle/src/state` for the default
//! (non-`jupiter`) build deployed at `BUNDDh4P…`. Every reader checks the Anchor
//! account discriminator and bounds-checks each field, returning
//! `DeserializationFailed` rather than panicking on short or foreign data.

use solana_pubkey::Pubkey;

use crate::trading_venue::error::TradingVenueError;

/// `sha256("account:Bundle")[..8]`
pub const BUNDLE_DISCRIMINATOR: [u8; 8] = [15, 82, 167, 230, 37, 214, 82, 80];
/// `sha256("account:BundleTempData")[..8]`
pub const BUNDLE_TEMP_DATA_DISCRIMINATOR: [u8; 8] = [3, 137, 171, 33, 191, 15, 54, 4];
/// `sha256("account:OracleData")[..8]`
pub const ORACLE_DATA_DISCRIMINATOR: [u8; 8] = [26, 131, 25, 110, 6, 141, 10, 37];
/// `sha256("account:TokenizedBundleDepositor")[..8]`
pub const TOKENIZED_BUNDLE_DEPOSITOR_DISCRIMINATOR: [u8; 8] = [76, 78, 8, 31, 121, 235, 87, 100];

/// Sequential little-endian reader over Borsh-encoded account data.
struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], discriminator: &[u8; 8]) -> Result<Self, TradingVenueError> {
        if data.get(..8) != Some(discriminator.as_slice()) {
            return Err(TradingVenueError::DeserializationFailed(
                "account discriminator mismatch".into(),
            ));
        }
        Ok(Self { data, offset: 8 })
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], TradingVenueError> {
        let end = self
            .offset
            .checked_add(N)
            .ok_or(TradingVenueError::DeserializationFailed(
                "account offset overflow".into(),
            ))?;
        let bytes: [u8; N] = self
            .data
            .get(self.offset..end)
            .and_then(|s| s.try_into().ok())
            .ok_or(TradingVenueError::DeserializationFailed(
                "account data too short".into(),
            ))?;
        self.offset = end;
        Ok(bytes)
    }

    fn skip(&mut self, n: usize) -> Result<(), TradingVenueError> {
        let end = self
            .offset
            .checked_add(n)
            .filter(|end| *end <= self.data.len())
            .ok_or(TradingVenueError::DeserializationFailed(
                "account data too short".into(),
            ))?;
        self.offset = end;
        Ok(())
    }

    fn u8(&mut self) -> Result<u8, TradingVenueError> {
        Ok(self.take::<1>()?[0])
    }
    fn bool(&mut self) -> Result<bool, TradingVenueError> {
        Ok(self.u8()? != 0)
    }
    fn u32(&mut self) -> Result<u32, TradingVenueError> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn u64(&mut self) -> Result<u64, TradingVenueError> {
        Ok(u64::from_le_bytes(self.take()?))
    }
    fn i64(&mut self) -> Result<i64, TradingVenueError> {
        Ok(i64::from_le_bytes(self.take()?))
    }
    fn u128(&mut self) -> Result<u128, TradingVenueError> {
        Ok(u128::from_le_bytes(self.take()?))
    }
    fn i128(&mut self) -> Result<i128, TradingVenueError> {
        Ok(i128::from_le_bytes(self.take()?))
    }
    fn pubkey(&mut self) -> Result<Pubkey, TradingVenueError> {
        Ok(Pubkey::new_from_array(self.take()?))
    }
}

/// The `Bundle` fields the instant-mint path reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct Bundle {
    pub treasury_account: Pubkey,
    pub bundle_underlying_balance: u64,
    pub max_deposit_amount: u64,
    pub performance_fee: u32,
    pub management_fee_bps: u32,
    pub deposit_fee: u32,
    pub total_shares: u128,
    pub asset_precision: u64,
    pub asset_address: Pubkey,
    pub asset_decimals: u8,
    pub permissionned: bool,
    pub min_deposit_amount: u64,
    pub issuance_fee_bps: u32,
    pub last_management_fee_change_timestamp: i64,
    pub performance_fee_version: u64,
}

impl Bundle {
    pub fn load(data: &[u8]) -> Result<Self, TradingVenueError> {
        let mut r = Reader::new(data, &BUNDLE_DISCRIMINATOR)?;
        r.skip(32)?; // name
        r.skip(32)?; // manager
        r.skip(32)?; // keeper
        let treasury_account = r.pubkey()?;
        let receivers = r.u32()? as usize; // allocated_receivers: Vec<Pubkey>
        r.skip(
            receivers
                .checked_mul(32)
                .ok_or(TradingVenueError::DeserializationFailed(
                    "receiver count overflow".into(),
                ))?,
        )?;

        let bundle_underlying_balance = r.u64()?;
        let max_deposit_amount = r.u64()?;
        r.skip(8)?; // withdrawal_delay
        let performance_fee = r.u32()?;
        let management_fee_bps = r.u32()?;
        let deposit_fee = r.u32()?;
        r.skip(4)?; // withdrawal_fee
        r.skip(16)?; // manager_pfee_shares
        r.skip(4)?; // current_allocation_bps
        r.skip(8)?; // oracle_buffer
        let total_shares = r.u128()?;
        let asset_precision = r.u64()?;
        let asset_address = r.pubkey()?;
        let asset_decimals = r.u8()?;
        r.skip(8 + 8 + 4)?; // withdrawal_t_min, withdrawal_t_max, withdrawal_curve
        let permissionned = r.bool()?;
        r.skip(16)?; // manager_mfee_shares
        let min_deposit_amount = r.u64()?;
        r.skip(8 + 8)?; // oracle_update_time_limit, oracle_max_age
        r.skip(8 * 3)?; // withdrawal redemption schedule
        r.skip(1 + 8)?; // referrer_enabled, referrer_min_deposit_amount
        r.skip(16 * 5 + 1)?; // referral_tiers, tier_count
        let issuance_fee_bps = r.u32()?;
        let last_management_fee_change_timestamp = r.i64()?;
        r.skip(8)?; // management_fee_version
        let performance_fee_version = r.u64()?;

        Ok(Self {
            treasury_account,
            bundle_underlying_balance,
            max_deposit_amount,
            performance_fee,
            management_fee_bps,
            deposit_fee,
            total_shares,
            asset_precision,
            asset_address,
            asset_decimals,
            permissionned,
            min_deposit_amount,
            issuance_fee_bps,
            last_management_fee_change_timestamp,
            performance_fee_version,
        })
    }
}

/// The `BundleTempData` fields the instant-mint path reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct BundleTempData {
    pub cumulative_pending_deposits: u64,
    pub last_netting_timestamp: i64,
    pub is_distribution_started: bool,
    pub paused_deposits_withdrawals: bool,
}

impl BundleTempData {
    pub fn load(data: &[u8]) -> Result<Self, TradingVenueError> {
        let mut r = Reader::new(data, &BUNDLE_TEMP_DATA_DISCRIMINATOR)?;
        let cumulative_pending_deposits = r.u64()?;
        r.skip(8 * 3)?; // pending deposit / withdrawal / locked withdrawal counters
        r.skip(16)?; // last_total_shares_minted
        let last_netting_timestamp = r.i64()?;
        r.skip(1)?; // is_netting_done
        r.skip(8)?; // distribution_base_amount
        let is_distribution_started = r.bool()?;
        let paused_deposits_withdrawals = r.bool()?;
        Ok(Self {
            cumulative_pending_deposits,
            last_netting_timestamp,
            is_distribution_started,
            paused_deposits_withdrawals,
        })
    }
}

/// `OracleData::average_external_equity`, the off-vault half of total equity.
pub fn load_oracle_external_equity(data: &[u8]) -> Result<u64, TradingVenueError> {
    Reader::new(data, &ORACLE_DATA_DISCRIMINATOR)?.u64()
}

/// The share-token pool backing the bundle receipt token.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokenizedBundleDepositor {
    pub bundle: Pubkey,
    pub mint: Pubkey,
    pub token_authority: Pubkey,
    pub shares: u128,
    pub cumulative_taxed_profit: u128,
    pub last_management_fee_timestamp: i64,
    pub net_deposits: i128,
    pub mint_cap_per_cycle: u64,
    pub current_cycle_minted: u64,
    pub management_fee_remainder: u128,
    pub performance_fee_version: u64,
    pub current_cycle_netting_timestamp: i64,
}

impl TokenizedBundleDepositor {
    pub fn load(data: &[u8]) -> Result<Self, TradingVenueError> {
        let mut r = Reader::new(data, &TOKENIZED_BUNDLE_DEPOSITOR_DISCRIMINATOR)?;
        let bundle = r.pubkey()?;
        let mint = r.pubkey()?;
        let token_authority = r.pubkey()?;
        let shares = r.u128()?;
        let cumulative_taxed_profit = r.u128()?;
        let last_management_fee_timestamp = r.i64()?;
        let net_deposits = r.i128()?;
        r.skip(8)?; // total_fee_charged
        r.skip(3)?; // bump, mint_bump, token_authority_bump
        let mint_cap_per_cycle = r.u64()?;
        let current_cycle_minted = r.u64()?;
        let management_fee_remainder = r.u128()?;
        let performance_fee_version = r.u64()?;
        let current_cycle_netting_timestamp = r.i64()?;
        Ok(Self {
            bundle,
            mint,
            token_authority,
            shares,
            cumulative_taxed_profit,
            last_management_fee_timestamp,
            net_deposits,
            mint_cap_per_cycle,
            current_cycle_minted,
            management_fee_remainder,
            performance_fee_version,
            current_cycle_netting_timestamp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discriminator(name: &str) -> [u8; 8] {
        let hash = solana_sdk::hash::hash(format!("account:{name}").as_bytes());
        hash.to_bytes()[..8].try_into().unwrap()
    }

    #[test]
    fn discriminators_match_anchor() {
        assert_eq!(BUNDLE_DISCRIMINATOR, discriminator("Bundle"));
        assert_eq!(
            BUNDLE_TEMP_DATA_DISCRIMINATOR,
            discriminator("BundleTempData")
        );
        assert_eq!(ORACLE_DATA_DISCRIMINATOR, discriminator("OracleData"));
        assert_eq!(
            TOKENIZED_BUNDLE_DEPOSITOR_DISCRIMINATOR,
            discriminator("TokenizedBundleDepositor")
        );
    }

    #[test]
    fn readers_reject_foreign_and_short_data() {
        assert!(Bundle::load(&[0u8; 64]).is_err());
        assert!(Bundle::load(&BUNDLE_DISCRIMINATOR).is_err());
        assert!(TokenizedBundleDepositor::load(&TOKENIZED_BUNDLE_DEPOSITOR_DISCRIMINATOR).is_err());
        assert!(load_oracle_external_equity(&[]).is_err());
    }

    #[test]
    fn tokenized_depositor_round_trips_fixed_offsets() {
        // 8-byte discriminator + 327-byte InitSpace, as allocated on chain.
        let mut data = vec![0u8; 335];
        data[..8].copy_from_slice(&TOKENIZED_BUNDLE_DEPOSITOR_DISCRIMINATOR);
        data[104..120].copy_from_slice(&7u128.to_le_bytes()); // shares
        data[144..160].copy_from_slice(&(-3i128).to_le_bytes()); // net_deposits
        data[171..179].copy_from_slice(&11u64.to_le_bytes()); // mint_cap_per_cycle
        data[211..219].copy_from_slice(&13i64.to_le_bytes()); // current_cycle_netting_timestamp

        let pool = TokenizedBundleDepositor::load(&data).unwrap();
        assert_eq!(pool.shares, 7);
        assert_eq!(pool.net_deposits, -3);
        assert_eq!(pool.mint_cap_per_cycle, 11);
        assert_eq!(pool.current_cycle_netting_timestamp, 13);
    }
}
