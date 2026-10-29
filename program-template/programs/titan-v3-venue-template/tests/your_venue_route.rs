//! Neutral bundle swap-route test — the same end-to-end suite the example
//! passes, run against `NeutralBundleVenue`. SKIPs cleanly until SOLANA_RPC_URL,
//! the built route program, and `NEUTRAL_TEST_BUNDLE` (a mainnet bundle with a
//! tokenized depositor) are all available.

mod common;

use common::{run_swap_route, RouteConfig};
use solana_pubkey::Pubkey;
use titan_integration_template::your_venue::{NeutralBundleVenue, NEUTRAL_BUNDLE_PROGRAM_ID};

fn pool() -> Option<Pubkey> {
    let Ok(bundle) = std::env::var("NEUTRAL_TEST_BUNDLE") else {
        eprintln!(
            "SKIP swap_route_both_directions: set NEUTRAL_TEST_BUNDLE to a bundle with a \
             tokenized depositor"
        );
        return None;
    };
    Some(
        bundle
            .parse()
            .expect("NEUTRAL_TEST_BUNDLE must be a base58 pubkey"),
    )
}

fn venue_programs() -> Vec<Pubkey> {
    vec![NEUTRAL_BUNDLE_PROGRAM_ID]
}

#[tokio::test]
async fn swap_route_both_directions() {
    let Some(pool) = pool() else { return };
    run_swap_route::<NeutralBundleVenue>(RouteConfig {
        pool,
        venue_programs: venue_programs(),
    })
    .await;
}
