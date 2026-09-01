//! Integration tests for the v5 mint + redemption flows, executed against the
//! locally built OnRe v5 program (`onreapp.so`) in LiteSVM.
//!
//! The program binary is loaded at runtime from `ONREAPP_SO_PATH`, defaulting
//! to `../onre-sol/target/deploy/onreapp.so` relative to this repo (build it
//! with `anchor build` on the `master` branch of onre-sol).

mod v5_common;

use onre_titan::constants::*;
use onre_titan::errors::{classify_program_error, ExpectedFailure};
use onre_titan::prop_amm::{
    build_open_swap_sell_instruction, build_quote_swap_sell_instruction, parse_swap_sell_quote,
};
use onre_titan::redemption::{
    build_create_redemption_request_instruction, find_redemption_offer_pda,
    find_redemption_request_pda, redemption_request_status, RedemptionOffer, RedemptionRequest,
    RedemptionRequestStatus,
};
use onre_titan::trading_venue::{QuoteRequest, SwapType, VenueStatus};
use solana_instruction::AccountMeta;
use solana_sdk::signer::Signer;

use v5_common::*;

// ===========================================================================
// Mint flow (take_offer_permissionless_v2)
// ===========================================================================

#[test]
fn test_v2_mint_happy_path_quote_matches_onchain_result() {
    let mut ctx = setup_mint_offer();

    // Build the venue exactly like an integrator: load offer account, then
    // refresh state through the AccountsCache abstraction.
    let venue = ctx.load_venue();
    assert!(venue.initialized());
    assert_eq!(venue.status(), VenueStatus::Active);

    let amount = 1_000_000u64; // 1 USDC
    let quote = venue
        .quote(QuoteRequest {
            input_mint: ctx.usdc_mint,
            output_mint: ctx.onyc_mint,
            amount,
            swap_type: SwapType::ExactIn,
        })
        .unwrap();
    assert!(!quote.not_enough_liquidity);
    // price 1.0 (apr=0), 6 -> 9 decimals: 1 USDC mints exactly 1 ONyc
    assert_eq!(quote.expected_output, 1_000_000_000);

    let ix = venue
        .generate_swap_instruction_v2(
            QuoteRequest {
                input_mint: ctx.usdc_mint,
                output_mint: ctx.onyc_mint,
                amount,
                swap_type: SwapType::ExactIn,
            },
            ctx.user.pubkey(),
        )
        .unwrap();

    let user = ctx.user.insecure_clone();
    ctx.send_ixs(&[ix], &[&user])
        .expect("v2 take should succeed");

    let user_onyc = ctx.token_balance(&ctx.user.pubkey(), &ctx.onyc_mint);
    assert_eq!(user_onyc, quote.expected_output);

    let user_usdc = ctx.token_balance(&ctx.user.pubkey(), &ctx.usdc_mint);
    assert_eq!(user_usdc, USER_USDC_BALANCE - amount);
}

#[test]
fn test_v2_instruction_matches_final_canonical_layout() {
    let ctx = setup_mint_offer();
    let venue = ctx.load_venue();
    let amount = 1_000_000u64;
    let ix = venue
        .generate_swap_instruction_v2(
            QuoteRequest {
                input_mint: ctx.usdc_mint,
                output_mint: ctx.onyc_mint,
                amount,
                swap_type: SwapType::ExactIn,
            },
            ctx.user.pubkey(),
        )
        .unwrap();

    let mut expected_data = TAKE_OFFER_PERMISSIONLESS_V2_DISCRIMINATOR.to_vec();
    expected_data.extend_from_slice(&amount.to_le_bytes());
    assert_eq!(ix.data, expected_data);

    let vault_authority = pda(&[SEED_OFFER_VAULT_AUTHORITY]);
    let permissionless_authority = pda(&[SEED_PERMISSIONLESS_AUTHORITY]);
    let redemption_offer = pda(&[
        SEED_REDEMPTION_OFFER,
        ctx.onyc_mint.as_ref(),
        ctx.usdc_mint.as_ref(),
    ]);
    let redemption_vault_authority = pda(&[SEED_REDEMPTION_OFFER_VAULT_AUTHORITY]);
    let offer_proceeds_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_OFFER_PROCEEDS_VAULT]);
    let permissionless_fee_vault =
        pda(&[SEED_CONFIGURABLE_VAULT, SEED_PERMISSIONLESS_OFFER_FEE_VAULT]);
    let reserve_vault_authority = pda(&[SEED_RESERVE_VAULT_AUTHORITY]);
    let management_fee_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_MANAGEMENT_FEE_VAULT]);
    let performance_fee_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_PERFORMANCE_FEE_VAULT]);

    let expected_accounts = vec![
        AccountMeta::new(ctx.offer_pda, false),
        AccountMeta::new_readonly(pda(&[SEED_STATE]), false),
        AccountMeta::new_readonly(vault_authority, false),
        AccountMeta::new(derive_ata(&vault_authority, &ctx.usdc_mint), false),
        AccountMeta::new(derive_ata(&vault_authority, &ctx.onyc_mint), false),
        AccountMeta::new_readonly(permissionless_authority, false),
        AccountMeta::new(derive_ata(&permissionless_authority, &ctx.usdc_mint), false),
        AccountMeta::new(derive_ata(&permissionless_authority, &ctx.onyc_mint), false),
        AccountMeta::new(ctx.usdc_mint, false),
        AccountMeta::new_readonly(TOKEN_PROGRAM, false),
        AccountMeta::new(ctx.onyc_mint, false),
        AccountMeta::new_readonly(TOKEN_PROGRAM, false),
        AccountMeta::new(derive_ata(&ctx.user.pubkey(), &ctx.usdc_mint), false),
        AccountMeta::new(derive_ata(&ctx.user.pubkey(), &ctx.onyc_mint), false),
        AccountMeta::new_readonly(redemption_offer, false),
        AccountMeta::new_readonly(redemption_vault_authority, false),
        AccountMeta::new(
            derive_ata(&redemption_vault_authority, &ctx.usdc_mint),
            false,
        ),
        AccountMeta::new(offer_proceeds_vault, false),
        AccountMeta::new(derive_ata(&offer_proceeds_vault, &ctx.usdc_mint), false),
        AccountMeta::new(permissionless_fee_vault, false),
        AccountMeta::new(derive_ata(&permissionless_fee_vault, &ctx.usdc_mint), false),
        AccountMeta::new_readonly(pda(&[SEED_MINT_AUTHORITY]), false),
        AccountMeta::new(pda(&[SEED_BUFFER_STATE]), false),
        AccountMeta::new(derive_ata(&reserve_vault_authority, &ctx.onyc_mint), false),
        AccountMeta::new(derive_ata(&management_fee_vault, &ctx.onyc_mint), false),
        AccountMeta::new(derive_ata(&performance_fee_vault, &ctx.onyc_mint), false),
        AccountMeta::new(pda(&[SEED_MARKET_STATS]), false),
        AccountMeta::new_readonly(pda(&[SEED_CIRCULATING_SUPPLY_EXCLUDED_BALANCE]), false),
        AccountMeta::new(ctx.user.pubkey(), true),
        AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM, false),
        AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        AccountMeta::new_readonly(ctx.offer_pda, false),
    ];
    assert_eq!(ix.accounts, expected_accounts);
}

#[test]
fn test_v2_permissionless_mint_uses_permissionless_fee_and_vault() {
    let mut ctx = setup_mint_offer_with_fees(100, 300); // regular 1%, permissionless 3%

    let venue = ctx.load_venue();
    assert_eq!(venue.offer.fee_basis_points, 100);
    assert_eq!(venue.offer.fee_basis_points_permissionless, 300);

    let amount = 1_000_000u64;
    let quote = venue
        .quote(QuoteRequest {
            input_mint: ctx.usdc_mint,
            output_mint: ctx.onyc_mint,
            amount,
            swap_type: SwapType::ExactIn,
        })
        .unwrap();
    // The v2 permissionless path uses 3%, not the regular 1% fee.
    assert_eq!(quote.expected_output, 970_000_000);

    let ix = venue
        .generate_swap_instruction_v2(
            QuoteRequest {
                input_mint: ctx.usdc_mint,
                output_mint: ctx.onyc_mint,
                amount,
                swap_type: SwapType::ExactIn,
            },
            ctx.user.pubkey(),
        )
        .unwrap();
    let user = ctx.user.insecure_clone();
    ctx.send_ixs(&[ix], &[&user])
        .expect("v2 take should succeed");

    assert_eq!(
        ctx.token_balance(&ctx.user.pubkey(), &ctx.onyc_mint),
        quote.expected_output
    );

    let permissionless_fee_vault =
        pda(&[SEED_CONFIGURABLE_VAULT, SEED_PERMISSIONLESS_OFFER_FEE_VAULT]);
    assert_eq!(
        ctx.token_balance(&permissionless_fee_vault, &ctx.usdc_mint),
        30_000
    );

    // Regular execution has a different fee lane and must not receive the
    // permissionless fee.
    let regular_fee_vault = pda(&[SEED_CONFIGURABLE_VAULT, b"offer_fee"]);
    assert_eq!(
        ctx.token_balance_or_zero(&regular_fee_vault, &ctx.usdc_mint),
        0
    );

    let proceeds_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_OFFER_PROCEEDS_VAULT]);
    assert_eq!(ctx.token_balance(&proceeds_vault, &ctx.usdc_mint), 970_000);
}

// ===========================================================================
// Atomic RFQ sell flow (quote_swap_sell + open_swap_sell)
// ===========================================================================

#[test]
fn test_atomic_sell_quote_executes_against_funded_usdg_liquidity() {
    let mut ctx = setup_mint_offer();
    ctx.setup_redemption_offer_with_fees(20, 20);

    let boss = ctx.payer.insecure_clone();
    let configure_ix = build_configure_prop_amm_ix(&boss.pubkey(), &ctx.usdc_mint, &ctx.onyc_mint);
    ctx.send_ixs(&[configure_ix], &[&boss])
        .expect("configure_prop_amm failed");

    // LiteSVM 0.6 compatibility: the current program lazily initializes these
    // PDAs with SIMD-0312, covered by onre-sol's own LiteSVM 0.14 suite.
    let proceeds_vault = create_configurable_vault(&mut ctx.svm, SEED_PROP_AMM_PROCEEDS_VAULT, 5);
    let sell_fee_vault = create_configurable_vault(&mut ctx.svm, SEED_PROP_AMM_SELL_FEE_VAULT, 8);

    let redemption_vault_authority = pda(&[SEED_REDEMPTION_OFFER_VAULT_AUTHORITY]);
    let funded_liquidity = 40_000_000_000_000u64; // 40m units at 6 decimals
    create_token_account(
        &mut ctx.svm,
        &ctx.usdc_mint,
        &redemption_vault_authority,
        funded_liquidity,
    );

    let sell_amount = 10_000_000_000u64; // 10 ONyc at 9 decimals
    create_token_account(
        &mut ctx.svm,
        &ctx.onyc_mint,
        &ctx.user.pubkey(),
        sell_amount,
    );

    let quote_ix = build_quote_swap_sell_instruction(
        &ctx.onyc_mint,
        &ctx.usdc_mint,
        &TOKEN_PROGRAM,
        sell_amount,
    )
    .unwrap();
    let quote_metadata = ctx
        .send_ixs(&[quote_ix], &[&boss])
        .expect("quote_swap_sell failed");
    let quote = parse_swap_sell_quote(
        &quote_metadata.return_data.program_id,
        &quote_metadata.return_data.data,
        &ctx.onyc_mint,
        &ctx.usdc_mint,
        sell_amount,
    )
    .unwrap();
    assert_eq!(quote.token_in_fee_amount, 20_000_000); // 20 bps on-chain
    assert_eq!(quote.token_in_net_amount, 9_980_000_000);
    assert!(quote.minimum_out > 0);

    let user_usdg_before = ctx.token_balance(&ctx.user.pubkey(), &ctx.usdc_mint);
    let sell_ix = build_open_swap_sell_instruction(
        &ctx.user.pubkey(),
        &ctx.onyc_mint,
        &ctx.usdc_mint,
        &TOKEN_PROGRAM,
        &TOKEN_PROGRAM,
        &ctx.offer_pda,
        &quote,
    )
    .unwrap();
    let user = ctx.user.insecure_clone();
    ctx.send_ixs(&[sell_ix], &[&user])
        .expect("open_swap_sell failed");

    assert_eq!(
        ctx.token_balance(&ctx.user.pubkey(), &ctx.usdc_mint),
        user_usdg_before + quote.token_out_amount
    );
    assert_eq!(
        ctx.token_balance(&sell_fee_vault, &ctx.onyc_mint),
        quote.token_in_fee_amount
    );
    // The fixture leaves mint authority with the boss, so net ONyc is retained
    // in the proceeds vault instead of burned.
    assert_eq!(
        ctx.token_balance(&proceeds_vault, &ctx.onyc_mint),
        quote.token_in_net_amount
    );
    assert_eq!(
        ctx.token_balance(&redemption_vault_authority, &ctx.usdc_mint),
        funded_liquidity - quote.token_out_amount
    );
}

// ===========================================================================
// Redemption flow (create_redemption_request + status tracking)
// ===========================================================================

#[test]
fn test_create_redemption_request_and_read_back_state() {
    let mut ctx = setup_mint_offer();
    ctx.setup_redemption_offer_with_fees(20, 25);

    // User holds ONyc to redeem
    let user_onyc = 2_000_000_000u64;
    create_token_account(&mut ctx.svm, &ctx.onyc_mint, &ctx.user.pubkey(), user_onyc);

    // Integrator surface: read the redemption offer to get the counter
    let (redemption_offer_pda, _) = find_redemption_offer_pda(&ctx.onyc_mint, &ctx.usdc_mint);
    let ro_account = ctx.svm.get_account(&redemption_offer_pda).unwrap();
    let ro = RedemptionOffer::load(&ro_account.data).unwrap();
    assert_eq!(ro.request_counter, 0);
    assert_eq!(ro.fee_basis_points, 20);
    assert_eq!(ro.fee_basis_points_prop_amm_sell, 25);
    assert!(!ro.is_disabled());

    let amount = 1_500_000_000u64;
    let ix = build_create_redemption_request_instruction(
        &ctx.user.pubkey(),
        &ctx.onyc_mint,
        &ctx.usdc_mint,
        amount,
        ro.request_counter,
    );
    let user = ctx.user.insecure_clone();
    ctx.send_ixs(&[ix], &[&user])
        .expect("create_redemption_request should succeed");

    // Read back the request state
    let (request_pda, _) = find_redemption_request_pda(&redemption_offer_pda, 0);
    let request_account = ctx.svm.get_account(&request_pda).unwrap();
    let request = RedemptionRequest::load(&request_account.data).unwrap();
    assert_eq!(request.offer, redemption_offer_pda);
    assert_eq!(request.request_id, 0);
    assert_eq!(request.redeemer, ctx.user.pubkey());
    assert_eq!(request.amount, amount);
    assert_eq!(request.fulfilled_amount, 0);
    assert_eq!(
        redemption_request_status(Some(&request_account.data)).unwrap(),
        RedemptionRequestStatus::Pending
    );

    // The redemption offer advanced its counter and requested amount
    let ro_account = ctx.svm.get_account(&redemption_offer_pda).unwrap();
    let ro = RedemptionOffer::load(&ro_account.data).unwrap();
    assert_eq!(ro.request_counter, 1);
    assert_eq!(ro.requested_redemptions, amount as u128);

    // Tokens were locked in the redemption vault
    let vault_authority = pda(&[SEED_REDEMPTION_OFFER_VAULT_AUTHORITY]);
    assert_eq!(ctx.token_balance(&vault_authority, &ctx.onyc_mint), amount);
    assert_eq!(
        ctx.token_balance(&ctx.user.pubkey(), &ctx.onyc_mint),
        user_onyc - amount
    );
}

// ===========================================================================
// Error paths: disabled offer + kill switch as expected states
// ===========================================================================

#[test]
fn test_disabled_offer_is_surfaced_as_expected_state() {
    let mut ctx = setup_mint_offer();

    let boss = ctx.payer.insecure_clone();
    let ix = build_set_offer_disabled_ix(&boss.pubkey(), &ctx.usdc_mint, &ctx.onyc_mint, true);
    ctx.send_ixs(&[ix], &[&boss]).unwrap();

    // 1. The venue reports the disabled state and quotes no liquidity
    let venue = ctx.load_venue();
    assert_eq!(venue.status(), VenueStatus::OfferDisabled);
    let quote = venue
        .quote(QuoteRequest {
            input_mint: ctx.usdc_mint,
            output_mint: ctx.onyc_mint,
            amount: 1_000_000,
            swap_type: SwapType::ExactIn,
        })
        .unwrap();
    assert!(quote.not_enough_liquidity);

    // 2. If a take is submitted anyway, the failure classifies as OfferDisabled
    let ix = venue
        .generate_swap_instruction_v2(
            QuoteRequest {
                input_mint: ctx.usdc_mint,
                output_mint: ctx.onyc_mint,
                amount: 1_000_000,
                swap_type: SwapType::ExactIn,
            },
            ctx.user.pubkey(),
        )
        .unwrap();
    let user = ctx.user.insecure_clone();
    let err = ctx.send_ixs(&[ix], &[&user]).unwrap_err();
    let code = custom_error_code(&err).expect("expected a custom program error");
    assert_eq!(
        classify_program_error(code),
        Some(ExpectedFailure::OfferDisabled)
    );
}

#[test]
fn test_kill_switch_is_surfaced_as_expected_state() {
    let mut ctx = setup_mint_offer();
    ctx.setup_redemption_offer();
    create_token_account(
        &mut ctx.svm,
        &ctx.onyc_mint,
        &ctx.user.pubkey(),
        1_000_000_000,
    );

    let boss = ctx.payer.insecure_clone();
    let ix = build_set_kill_switch_ix(&boss.pubkey(), true);
    ctx.send_ixs(&[ix], &[&boss]).unwrap();

    // 1. Venue state reflects the kill switch
    let venue = ctx.load_venue();
    assert_eq!(venue.status(), VenueStatus::KillSwitchActive);
    let quote = venue
        .quote(QuoteRequest {
            input_mint: ctx.usdc_mint,
            output_mint: ctx.onyc_mint,
            amount: 1_000_000,
            swap_type: SwapType::ExactIn,
        })
        .unwrap();
    assert!(quote.not_enough_liquidity);

    // 2. Mint path fails with the classified kill-switch error
    let ix = venue
        .generate_swap_instruction_v2(
            QuoteRequest {
                input_mint: ctx.usdc_mint,
                output_mint: ctx.onyc_mint,
                amount: 1_000_000,
                swap_type: SwapType::ExactIn,
            },
            ctx.user.pubkey(),
        )
        .unwrap();
    let user = ctx.user.insecure_clone();
    let err = ctx.send_ixs(&[ix], &[&user]).unwrap_err();
    assert_eq!(
        custom_error_code(&err).and_then(classify_program_error),
        Some(ExpectedFailure::KillSwitchActivated)
    );

    // 3. Redemption path fails with the same classified state
    let ix = build_create_redemption_request_instruction(
        &ctx.user.pubkey(),
        &ctx.onyc_mint,
        &ctx.usdc_mint,
        1_000_000_000,
        0,
    );
    let err = ctx.send_ixs(&[ix], &[&user]).unwrap_err();
    assert_eq!(
        custom_error_code(&err).and_then(classify_program_error),
        Some(ExpectedFailure::KillSwitchActivated)
    );
}

#[test]
fn test_disabled_redemption_offer_is_surfaced_as_expected_state() {
    let mut ctx = setup_mint_offer();
    ctx.setup_redemption_offer();
    create_token_account(
        &mut ctx.svm,
        &ctx.onyc_mint,
        &ctx.user.pubkey(),
        1_000_000_000,
    );

    let boss = ctx.payer.insecure_clone();
    let ix = build_set_redemption_offer_disabled_ix(
        &boss.pubkey(),
        &ctx.onyc_mint,
        &ctx.usdc_mint,
        true,
    );
    ctx.send_ixs(&[ix], &[&boss]).unwrap();

    // Integrator can read the disabled flag off the account
    let (redemption_offer_pda, _) = find_redemption_offer_pda(&ctx.onyc_mint, &ctx.usdc_mint);
    let ro_account = ctx.svm.get_account(&redemption_offer_pda).unwrap();
    assert!(RedemptionOffer::load(&ro_account.data)
        .unwrap()
        .is_disabled());

    // Submitting anyway fails with the classified state
    let ix = build_create_redemption_request_instruction(
        &ctx.user.pubkey(),
        &ctx.onyc_mint,
        &ctx.usdc_mint,
        1_000_000_000,
        0,
    );
    let user = ctx.user.insecure_clone();
    let err = ctx.send_ixs(&[ix], &[&user]).unwrap_err();
    assert_eq!(
        custom_error_code(&err).and_then(classify_program_error),
        Some(ExpectedFailure::RedemptionOfferDisabled)
    );
}
