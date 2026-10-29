//! Neutral bundle venue-creation parsing test.
//!
//! A bundle becomes routable when its manager runs
//! `initialize_tokenized_bundle_depositor`. The fixture uses the real mainnet
//! LighterLP-USDC bundle, its USDC asset, and the receipt mint PDA that
//! instruction would create (`[b"TOKENIZED_BUNDLE_MINT", bundle]`). Those PDAs
//! are mainnet-specific, so the fixture is skipped under the `devnet` feature.
#![cfg(not(feature = "devnet"))]

use solana_pubkey::{Pubkey, pubkey};

use titan_integration_template::trading_venue::protocol::PoolProtocol;
use titan_integration_template::trading_venue::venue_creation::{ParsedInstruction, PoolCreation};
use titan_integration_template::your_venue::{
    INITIALIZE_TOKENIZED_BUNDLE_DEPOSITOR_DISCRIMINATOR, NEUTRAL_BUNDLE_PROGRAM_ID,
    parse_pool_creations,
};

const MANAGER: Pubkey = pubkey!("2VYioVd8gaE1Zp1sMbo1u48pSLSnBsX6da4xxQs1MAMp");
const POOL: Pubkey = pubkey!("dvsWTBqogw1cou8ducadzc3YCmUssjrrio8LmMtNWmz");
const TOKENIZED_DEPOSITOR: Pubkey = pubkey!("138JQCARRWDbbnRAVGgvY4s5GrAQqj64MekWWkLrxwVb");
const TOKEN_AUTHORITY: Pubkey = pubkey!("91zS1gTwYPVBzF1WRQMqruJ68UBPA2bRQMSCfDbsv9Em");
/// USDC, the bundle asset.
const TOKEN_A_MINT: Pubkey = pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
/// The bundle receipt-token mint.
const TOKEN_B_MINT: Pubkey = pubkey!("8WjBC4GzAbpXTg2purATvgGmjJZDtBaoVaMdwJnBBfEt");

fn your_venue_pool_creation() -> ParsedInstruction {
    let mut data = INITIALIZE_TOKENIZED_BUNDLE_DEPOSITOR_DISCRIMINATOR.to_vec();
    data.push(6); // decimals
    ParsedInstruction {
        program_id: NEUTRAL_BUNDLE_PROGRAM_ID,
        accounts: vec![
            MANAGER,
            POOL,
            TOKEN_A_MINT,
            TOKENIZED_DEPOSITOR,
            TOKEN_AUTHORITY,
            TOKEN_B_MINT,
            spl_token::ID,
            solana_program::system_program::ID,
        ],
        data,
    }
}

fn unrelated_instruction() -> ParsedInstruction {
    ParsedInstruction {
        program_id: NEUTRAL_BUNDLE_PROGRAM_ID,
        accounts: vec![],
        data: vec![],
    }
}

#[test]
fn fixture_pdas_match_the_bundle() {
    let pda = |seed: &[u8]| {
        Pubkey::find_program_address(&[seed, POOL.as_ref()], &NEUTRAL_BUNDLE_PROGRAM_ID).0
    };
    assert_eq!(pda(b"TOKENIZED_BUNDLE_DEPOSITOR"), TOKENIZED_DEPOSITOR);
    assert_eq!(pda(b"TOKENIZED_BUNDLE_TOKEN_AUTHORITY"), TOKEN_AUTHORITY);
    assert_eq!(pda(b"TOKENIZED_BUNDLE_MINT"), TOKEN_B_MINT);
}

#[test]
fn parses_your_venue_pool_creation() {
    let creations = parse_pool_creations(&[your_venue_pool_creation()]);

    assert_eq!(
        creations,
        vec![PoolCreation {
            protocol: PoolProtocol::NeutralBundle,
            pool: POOL,
            mints: vec![TOKEN_A_MINT, TOKEN_B_MINT],
        }],
    );
}

#[test]
fn ignores_transactions_without_a_creation() {
    let creations = parse_pool_creations(&[unrelated_instruction()]);
    assert!(
        creations.is_empty(),
        "a transaction without a pool creation creates no pools, got {creations:?}"
    );
}
