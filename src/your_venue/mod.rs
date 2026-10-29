//! Neutral Trade bundle (`ntbundle`) integration.
//!
//! A bundle is a managed vault whose tokenized share pool issues an SPL receipt
//! token. The only atomic, permissionless swap the program exposes is
//! `instant_mint_bundle_tokens`: deposit the bundle asset, receive receipt
//! tokens in the same instruction. Redemption (`request_token_withdrawal`) is
//! queued and settled by a keeper in a later netting cycle, so it cannot be
//! routed. This venue therefore quotes a single direction:
//!
//! ```text
//! asset (index 0)  ->  bundle receipt token (index 1)
//! ```
//!
//! The quote replays the on-chain math exactly: deposit fee, bundle and cycle
//! caps, the pool's management/performance fee crank at the current clock, the
//! issuance fee, the share conversion, and the virtual-offset token conversion.

use ahash::HashSet;
use async_trait::async_trait;
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_program::clock::Clock;
use solana_program::sysvar::clock as clock_sysvar;
use solana_program_pack::Pack;
use solana_pubkey::Pubkey;
use spl_associated_token_account::get_associated_token_address_with_program_id;

use crate::{
    account_caching::AccountsCache,
    trading_venue::{
        AddressLookupTableTrait, FromAccount, QuoteRequest, QuoteResult, SwapType, TradingVenue,
        error::TradingVenueError,
        protocol::PoolProtocol,
        token_info::TokenInfo,
        venue_creation::{ParsedInstruction, PoolCreation},
    },
};

pub mod math;
pub mod state;

use math::{MintCurve, MintRejection, pool_shares_after_fees};
use state::{Bundle, BundleTempData, TokenizedBundleDepositor, load_oracle_external_equity};

/// `ntbundle` mainnet deployment (default, non-`jupiter` build).
#[cfg(not(feature = "devnet"))]
pub const NEUTRAL_BUNDLE_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("BUNDDh4P5XviMm1f3gCvnq2qKx6TGosAGnoUK12e7cXU");
/// `ntbundle` devnet deployment; same account layout as mainnet.
#[cfg(feature = "devnet")]
pub const NEUTRAL_BUNDLE_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("CcR9whVnaW3STx6LLYotwy5JJXmnC8KDjtRotA3NCL8v");

pub const BUNDLE_ASSET_AUTHORITY_SEED: &[u8] = b"BUNDLE_ASSET_AUTHORITY";
pub const ORACLE_SEED: &[u8] = b"ORACLE";
pub const BUNDLE_TEMP_DATA_SEED: &[u8] = b"BUNDLE_TEMP_DATA";
pub const TOKENIZED_BUNDLE_DEPOSITOR_SEED: &[u8] = b"TOKENIZED_BUNDLE_DEPOSITOR";
pub const TOKENIZED_BUNDLE_MINT_SEED: &[u8] = b"TOKENIZED_BUNDLE_MINT";
pub const TOKENIZED_BUNDLE_TOKEN_AUTHORITY_SEED: &[u8] = b"TOKENIZED_BUNDLE_TOKEN_AUTHORITY";

/// `sha256("global:instant_mint_bundle_tokens")[..8]`
pub const INSTANT_MINT_BUNDLE_TOKENS_DISCRIMINATOR: [u8; 8] = [56, 192, 88, 145, 75, 250, 148, 33];
/// `sha256("global:initialize_tokenized_bundle_depositor")[..8]`
pub const INITIALIZE_TOKENIZED_BUNDLE_DEPOSITOR_DISCRIMINATOR: [u8; 8] =
    [56, 137, 71, 110, 14, 153, 246, 241];

/// Detect every bundle that became mintable in a confirmed transaction.
///
/// A bundle becomes a venue when its manager calls
/// `initialize_tokenized_bundle_depositor`, which creates the receipt mint.
/// Account order (see `InitializeTokenizedBundleDepositor`):
/// `[manager, bundle, asset_mint, tokenized_depositor, token_authority, mint,
/// token_program, system_program]`.
pub fn parse_pool_creations(instructions: &[ParsedInstruction]) -> Vec<PoolCreation> {
    const BUNDLE_INDEX: usize = 1;
    const ASSET_MINT_INDEX: usize = 2;
    const RECEIPT_MINT_INDEX: usize = 5;

    instructions
        .iter()
        .filter(|ix| ix.program_id == NEUTRAL_BUNDLE_PROGRAM_ID)
        .filter(|ix| ix.data.get(..8) == Some(&INITIALIZE_TOKENIZED_BUNDLE_DEPOSITOR_DISCRIMINATOR))
        .filter_map(|ix| {
            let pool = *ix.accounts.get(BUNDLE_INDEX)?;
            let asset_mint = *ix.accounts.get(ASSET_MINT_INDEX)?;
            let receipt_mint = *ix.accounts.get(RECEIPT_MINT_INDEX)?;
            Some(PoolCreation {
                protocol: PoolProtocol::NeutralBundle,
                pool,
                mints: vec![asset_mint, receipt_mint],
            })
        })
        .collect()
}

fn bundle_pda(seed: &[u8], bundle: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[seed, bundle.as_ref()], &NEUTRAL_BUNDLE_PROGRAM_ID).0
}

/// Addresses derived from the bundle key.
#[derive(Clone, Copy, Debug)]
pub struct BundleKeys {
    pub bundle: Pubkey,
    pub temp_data: Pubkey,
    pub oracle: Pubkey,
    pub tokenized_depositor: Pubkey,
    pub mint: Pubkey,
    pub token_authority: Pubkey,
    pub asset_authority: Pubkey,
    pub asset_mint: Pubkey,
    /// The bundle's asset vault: the asset ATA of `asset_authority`.
    pub asset_vault: Pubkey,
    pub treasury: Pubkey,
}

impl BundleKeys {
    fn derive(bundle_key: Pubkey, bundle: &Bundle) -> Self {
        let asset_authority = bundle_pda(BUNDLE_ASSET_AUTHORITY_SEED, &bundle_key);
        Self {
            bundle: bundle_key,
            temp_data: bundle_pda(BUNDLE_TEMP_DATA_SEED, &bundle_key),
            oracle: bundle_pda(ORACLE_SEED, &bundle_key),
            tokenized_depositor: bundle_pda(TOKENIZED_BUNDLE_DEPOSITOR_SEED, &bundle_key),
            mint: bundle_pda(TOKENIZED_BUNDLE_MINT_SEED, &bundle_key),
            token_authority: bundle_pda(TOKENIZED_BUNDLE_TOKEN_AUTHORITY_SEED, &bundle_key),
            asset_authority,
            asset_mint: bundle.asset_address,
            asset_vault: get_associated_token_address_with_program_id(
                &asset_authority,
                &bundle.asset_address,
                &spl_token::ID,
            ),
            treasury: bundle.treasury_account,
        }
    }
}

/// Titan venue for minting a Neutral bundle's receipt token from its asset.
#[derive(Clone)]
pub struct NeutralBundleVenue {
    pub keys: BundleKeys,
    pub bundle: Bundle,
    pub curve: MintCurve,
    /// Unix timestamp of the clock sysvar the fee crank was evaluated at.
    pub clock_unix_timestamp: i64,
    /// Why the bundle cannot mint right now (paused, distributing, ...).
    halted: Option<&'static str>,
    /// `[asset, receipt token]`, populated in `update_state`.
    token_info: Vec<TokenInfo>,
    required_state_pubkeys: HashSet<Pubkey>,
    initialized: bool,
}

impl FromAccount for NeutralBundleVenue {
    fn from_account(pubkey: &Pubkey, account: &Account) -> Result<Self, TradingVenueError> {
        if account.owner != NEUTRAL_BUNDLE_PROGRAM_ID {
            return Err(TradingVenueError::FromAccountError(pubkey.into()));
        }
        let bundle = Bundle::load(&account.data)?;
        let keys = BundleKeys::derive(*pubkey, &bundle);
        let required_state_pubkeys = HashSet::from_iter([
            keys.bundle,
            keys.temp_data,
            keys.oracle,
            keys.tokenized_depositor,
            keys.mint,
            keys.asset_mint,
            clock_sysvar::ID,
        ]);

        Ok(Self {
            keys,
            bundle,
            curve: MintCurve::default(),
            clock_unix_timestamp: 0,
            halted: None,
            token_info: Vec::new(),
            required_state_pubkeys,
            initialized: false,
        })
    }
}

impl NeutralBundleVenue {
    fn is_mint_direction(&self, request: &QuoteRequest) -> bool {
        request.input_mint == self.keys.asset_mint && request.output_mint == self.keys.mint
    }

    fn check_request(&self, request: &QuoteRequest) -> Result<(), TradingVenueError> {
        if !self.is_mint_direction(request) {
            return Err(TradingVenueError::InvalidMint(request.input_mint.into()));
        }
        if request.swap_type != SwapType::ExactIn {
            return Err(TradingVenueError::ExactOutNotSupported);
        }
        Ok(())
    }
}

fn account<'a>(
    accounts: &'a [Option<Account>],
    i: usize,
    key: &Pubkey,
) -> Result<&'a Account, TradingVenueError> {
    accounts
        .get(i)
        .and_then(Option::as_ref)
        .ok_or(TradingVenueError::NoAccountFound(key.into()))
}

#[async_trait]
impl TradingVenue for NeutralBundleVenue {
    fn initialized(&self) -> bool {
        self.initialized
    }

    fn program_id(&self) -> Pubkey {
        NEUTRAL_BUNDLE_PROGRAM_ID
    }

    fn program_dependencies(&self) -> Vec<Pubkey> {
        vec![NEUTRAL_BUNDLE_PROGRAM_ID, spl_token::ID]
    }

    fn market_id(&self) -> Pubkey {
        self.keys.bundle
    }

    fn get_token_info(&self) -> &[TokenInfo] {
        &self.token_info
    }

    /// Mint only: asset (0) -> receipt token (1). Redemption is not atomic.
    fn directions_num(&self) -> Vec<(u8, u8)> {
        vec![(0, 1)]
    }

    fn protocol(&self) -> PoolProtocol {
        PoolProtocol::NeutralBundle
    }

    fn get_required_pubkeys_for_update(&self) -> Result<Vec<Pubkey>, TradingVenueError> {
        Ok(self.required_state_pubkeys.iter().cloned().collect())
    }

    async fn update_state(&mut self, cache: &dyn AccountsCache) -> Result<(), TradingVenueError> {
        let keys = [
            self.keys.bundle,
            self.keys.temp_data,
            self.keys.oracle,
            self.keys.tokenized_depositor,
            self.keys.mint,
            self.keys.asset_mint,
            clock_sysvar::ID,
        ];
        let accounts = cache.get_accounts(&keys).await?;

        let bundle = Bundle::load(&account(&accounts, 0, &keys[0])?.data)?;
        let temp = BundleTempData::load(&account(&accounts, 1, &keys[1])?.data)?;
        let external_equity = load_oracle_external_equity(&account(&accounts, 2, &keys[2])?.data)?;
        let pool = TokenizedBundleDepositor::load(&account(&accounts, 3, &keys[3])?.data)?;
        let mint_account = account(&accounts, 4, &keys[4])?;
        let asset_mint_account = account(&accounts, 5, &keys[5])?;
        let clock: Clock = bincode::deserialize(&account(&accounts, 6, &keys[6])?.data)
            .map_err(|_| TradingVenueError::DeserializationFailed(clock_sysvar::ID.into()))?;

        if pool.bundle != self.keys.bundle || pool.mint != self.keys.mint {
            return Err(TradingVenueError::DeserializationFailed(
                "tokenized depositor does not belong to this bundle".into(),
            ));
        }
        // `instant_mint_bundle_tokens` takes both mints as SPL Token accounts.
        if asset_mint_account.owner != spl_token::ID || mint_account.owner != spl_token::ID {
            return Err(TradingVenueError::UnsupportedVenue(
                "ntbundle instant mint supports SPL Token mints only".into(),
            ));
        }
        let mint = spl_token::state::Mint::unpack(&mint_account.data)
            .map_err(|_| TradingVenueError::DeserializationFailed(self.keys.mint.into()))?;
        let asset_mint = spl_token::state::Mint::unpack(&asset_mint_account.data)
            .map_err(|_| TradingVenueError::DeserializationFailed(self.keys.asset_mint.into()))?;

        let total_assets = bundle
            .bundle_underlying_balance
            .checked_add(external_equity)
            .ok_or(TradingVenueError::CheckedMathError(
                "total equity overflow".into(),
            ))?;
        let pool_shares = pool_shares_after_fees(
            &bundle,
            &pool,
            total_assets,
            mint.supply,
            clock.unix_timestamp,
        )?;
        let cycle_minted = if pool.current_cycle_netting_timestamp == temp.last_netting_timestamp {
            pool.current_cycle_minted
        } else {
            0
        };

        self.halted = if temp.paused_deposits_withdrawals {
            Some("bundle deposits and withdrawals are paused")
        } else if temp.is_distribution_started {
            Some("bundle distribution in progress")
        } else if bundle.permissionned {
            Some("permissioned bundles cannot be tokenized")
        } else if asset_mint.decimals != bundle.asset_decimals
            || mint.decimals != bundle.asset_decimals
        {
            Some("bundle asset precision mismatch")
        } else if mint.supply != 0 && pool_shares == 0 {
            Some("tokenized bundle depositor state invalid")
        } else {
            None
        };

        self.curve = MintCurve {
            deposit_fee_bps: bundle.deposit_fee,
            issuance_fee_bps: bundle.issuance_fee_bps,
            min_deposit_amount: bundle.min_deposit_amount,
            total_assets,
            total_shares: bundle.total_shares,
            committed_assets: total_assets
                .checked_add(temp.cumulative_pending_deposits)
                .ok_or(TradingVenueError::CheckedMathError(
                    "pending deposits overflow".into(),
                ))?,
            max_deposit_amount: bundle.max_deposit_amount,
            cycle_minted,
            mint_cap_per_cycle: pool.mint_cap_per_cycle,
            token_supply: mint.supply,
            pool_shares,
        };
        self.bundle = bundle;
        self.keys = BundleKeys::derive(self.keys.bundle, &bundle);
        self.clock_unix_timestamp = clock.unix_timestamp;
        self.token_info = vec![
            TokenInfo::new(&self.keys.asset_mint, asset_mint_account, clock.epoch)?,
            TokenInfo::new(&self.keys.mint, mint_account, clock.epoch)?,
        ];
        self.initialized = true;
        Ok(())
    }

    fn quote(&self, request: QuoteRequest) -> Result<QuoteResult, TradingVenueError> {
        self.check_request(&request)?;
        if !self.initialized {
            return Err(TradingVenueError::NotInitialized(self.keys.bundle.into()));
        }
        if let Some(reason) = self.halted {
            return Err(TradingVenueError::AmmMethodError(reason.into()));
        }

        let price = self.curve.price();
        let (expected_output, not_enough_liquidity) = if request.amount == 0 {
            (0, false)
        } else {
            match self.curve.tokens_out(request.amount)? {
                Ok(tokens) => (tokens, false),
                Err(MintRejection::TooSmall) => (0, false),
                Err(MintRejection::CapacityExceeded) => (0, true),
            }
        };

        Ok(QuoteResult {
            input_mint: request.input_mint,
            output_mint: request.output_mint,
            amount: request.amount,
            expected_output,
            not_enough_liquidity,
            price,
        })
    }

    fn generate_swap_instruction(
        &self,
        request: QuoteRequest,
        user: Pubkey,
    ) -> Result<Instruction, TradingVenueError> {
        self.check_request(&request)?;
        let keys = &self.keys;
        let user_asset_account =
            get_associated_token_address_with_program_id(&user, &keys.asset_mint, &spl_token::ID);
        let user_receipt_account =
            get_associated_token_address_with_program_id(&user, &keys.mint, &spl_token::ID);

        // Account order of `InstantMintBundleTokens`.
        let accounts = vec![
            AccountMeta::new(user, true),
            AccountMeta::new(user_asset_account, false),
            AccountMeta::new(keys.asset_vault, false),
            AccountMeta::new_readonly(keys.asset_authority, false),
            AccountMeta::new(keys.treasury, false),
            AccountMeta::new(keys.bundle, false),
            AccountMeta::new_readonly(keys.temp_data, false),
            AccountMeta::new_readonly(keys.oracle, false),
            AccountMeta::new(keys.tokenized_depositor, false),
            AccountMeta::new_readonly(keys.asset_mint, false),
            AccountMeta::new(keys.mint, false),
            AccountMeta::new_readonly(keys.token_authority, false),
            AccountMeta::new(user_receipt_account, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ];

        // `min_tokens_out = 0`: Titan enforces slippage on the whole route.
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&INSTANT_MINT_BUNDLE_TOKENS_DISCRIMINATOR);
        data.extend_from_slice(&request.amount.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes());

        Ok(Instruction {
            program_id: NEUTRAL_BUNDLE_PROGRAM_ID,
            accounts,
            data,
        })
    }
}

#[async_trait]
impl AddressLookupTableTrait for NeutralBundleVenue {
    async fn get_lookup_table_keys(
        &self,
        _accounts_cache: Option<&dyn AccountsCache>,
    ) -> Result<Vec<Pubkey>, TradingVenueError> {
        let keys = &self.keys;
        Ok(vec![
            NEUTRAL_BUNDLE_PROGRAM_ID,
            spl_token::ID,
            keys.bundle,
            keys.temp_data,
            keys.oracle,
            keys.tokenized_depositor,
            keys.mint,
            keys.token_authority,
            keys.asset_mint,
            keys.asset_authority,
            keys.asset_vault,
            keys.treasury,
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discriminator(name: &str) -> [u8; 8] {
        let hash = solana_sdk::hash::hash(format!("global:{name}").as_bytes());
        hash.to_bytes()[..8].try_into().unwrap()
    }

    #[test]
    fn instruction_discriminators_match_anchor() {
        assert_eq!(
            INSTANT_MINT_BUNDLE_TOKENS_DISCRIMINATOR,
            discriminator("instant_mint_bundle_tokens")
        );
        assert_eq!(
            INITIALIZE_TOKENIZED_BUNDLE_DEPOSITOR_DISCRIMINATOR,
            discriminator("initialize_tokenized_bundle_depositor")
        );
    }
}
