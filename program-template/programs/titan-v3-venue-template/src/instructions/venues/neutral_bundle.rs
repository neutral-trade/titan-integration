use anchor_lang::{prelude::*, solana_program::instruction::Instruction};

/// Neutral Trade `ntbundle` mainnet program.
#[cfg(not(feature = "devnet"))]
pub const PROGRAM_ID: Pubkey = pubkey!("BUNDDh4P5XviMm1f3gCvnq2qKx6TGosAGnoUK12e7cXU");
/// Neutral Trade `ntbundle` devnet program.
#[cfg(feature = "devnet")]
pub const PROGRAM_ID: Pubkey = pubkey!("CcR9whVnaW3STx6LLYotwy5JJXmnC8KDjtRotA3NCL8v");

/// `sha256("global:instant_mint_bundle_tokens")[..8]`
const INSTANT_MINT_BUNDLE_TOKENS_DISCRIMINATOR: [u8; 8] = [56, 192, 88, 145, 75, 250, 148, 33];

/// `instant_mint_bundle_tokens(amount_in, min_tokens_out)`: deposit the bundle
/// asset from the TitanPDA and mint receipt tokens to it. `min_tokens_out` is
/// zero because the router enforces slippage on the whole route.
pub fn instant_mint(amount_in: u64, account_metas: &[AccountMeta]) -> Result<Vec<Instruction>> {
    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&INSTANT_MINT_BUNDLE_TOKENS_DISCRIMINATOR);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());

    Ok(vec![Instruction {
        program_id: PROGRAM_ID,
        accounts: account_metas.to_vec(),
        data,
    }])
}
