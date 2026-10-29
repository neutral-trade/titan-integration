//! Neutral bundle test suite — the same shared assertions the example passes,
//! run against `NeutralBundleVenue`.
//!
//! Like the example suite, the tests SKIP when `SOLANA_RPC_URL` (and, for the
//! simulations, dumped program binaries) are absent. They also SKIP until
//! `NEUTRAL_TEST_BUNDLE` names a mainnet bundle that has a tokenized depositor
//! (`initialize_tokenized_bundle_depositor` has run), since no bundle on
//! mainnet has one yet.

mod common;

use common::SuiteConfig;
use solana_pubkey::Pubkey;
use titan_integration_template::your_venue::{NEUTRAL_BUNDLE_PROGRAM_ID, NeutralBundleVenue};

// Installs the allocation guard that powers the construction test's
// `assert_no_alloc` checks. The Makefile runs that test under `release-debug`
// so the guard is active; speed tests run under true `--release`.
#[cfg(debug_assertions)]
#[global_allocator]
static A: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

/// Tokenized bundle to quote, from `NEUTRAL_TEST_BUNDLE`.
fn pool() -> Option<Pubkey> {
    let Ok(bundle) = std::env::var("NEUTRAL_TEST_BUNDLE") else {
        eprintln!(
            "SKIP {}: set NEUTRAL_TEST_BUNDLE to a bundle with a tokenized depositor",
            std::thread::current().name().unwrap_or("a venue test")
        );
        return None;
    };
    Some(
        bundle
            .parse()
            .expect("NEUTRAL_TEST_BUNDLE must be a base58 pubkey"),
    )
}

fn programs() -> Vec<Pubkey> {
    // SPL Token is built into LiteSVM.
    vec![NEUTRAL_BUNDLE_PROGRAM_ID]
}

fn config() -> Option<SuiteConfig> {
    Some(SuiteConfig {
        pool: pool()?,
        programs: programs(),
    })
}

#[tokio::test]
async fn construction() {
    let Some(config) = config() else { return };
    common::construction::<NeutralBundleVenue>(&config).await;
}

#[tokio::test]
async fn zero_input_spot_price() {
    let Some(config) = config() else { return };
    common::zero_input_spot_price::<NeutralBundleVenue>(&config).await;
}

#[tokio::test]
async fn bound_simulation() {
    let Some(config) = config() else { return };
    common::bound_simulation::<NeutralBundleVenue>(&config).await;
}

#[tokio::test]
async fn random_samples() {
    let Some(config) = config() else { return };
    common::random_samples::<NeutralBundleVenue>(&config).await;
}

#[tokio::test]
async fn monotone() {
    let Some(config) = config() else { return };
    common::monotone::<NeutralBundleVenue>(&config).await;
}

#[tokio::test]
async fn quoting_speed() {
    let Some(config) = config() else { return };
    common::quoting_speed::<NeutralBundleVenue>(&config).await;
}

#[tokio::test]
async fn price_monotone() {
    let Some(config) = config() else { return };
    common::price_monotone::<NeutralBundleVenue>(&config).await;
}

#[tokio::test]
async fn mean_value_theorem() {
    let Some(config) = config() else { return };
    common::mean_value_theorem::<NeutralBundleVenue>(&config).await;
}
