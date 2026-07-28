//! Integration tests for the v5 mint + redemption flows, executed against the
//! locally built OnRe v5 program (`onreapp.so`) in LiteSVM.
//!
//! The program binary is loaded at runtime from `ONREAPP_SO_PATH`, defaulting
//! to `../onre-sol/target/deploy/onreapp.so` relative to this repo (build it
//! with `anchor build` on the `programV5` branch of onre-sol).

mod v5_common;

use onre_titan::constants::*;
use onre_titan::errors::{classify_program_error, ExpectedFailure};
use onre_titan::redemption::{
    build_create_redemption_request_instruction, find_redemption_offer_pda,
    find_redemption_request_pda, redemption_request_status, RedemptionOffer, RedemptionRequest,
    RedemptionRequestStatus,
};
use onre_titan::trading_venue::{QuoteRequest, SwapType, VenueStatus};
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
    ctx.send_ixs(&[ix], &[&user]).expect("v2 take should succeed");

    let user_onyc = ctx.token_balance(&ctx.user.pubkey(), &ctx.onyc_mint);
    assert_eq!(user_onyc, quote.expected_output);

    let user_usdc = ctx.token_balance(&ctx.user.pubkey(), &ctx.usdc_mint);
    assert_eq!(user_usdc, USER_USDC_BALANCE - amount);
}

#[test]
fn test_v2_mint_routes_fee_to_dedicated_fee_vault() {
    let mut ctx = setup_mint_offer_with_fee(100); // 1% fee

    let venue = ctx.load_venue();
    let amount = 1_000_000u64;
    let quote = venue
        .quote(QuoteRequest {
            input_mint: ctx.usdc_mint,
            output_mint: ctx.onyc_mint,
            amount,
            swap_type: SwapType::ExactIn,
        })
        .unwrap();
    // 1% fee: 990_000 net at price 1.0 -> 0.99 ONyc
    assert_eq!(quote.expected_output, 990_000_000);

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
    ctx.send_ixs(&[ix], &[&user]).expect("v2 take should succeed");

    assert_eq!(
        ctx.token_balance(&ctx.user.pubkey(), &ctx.onyc_mint),
        quote.expected_output
    );

    // The 1% fee lands in the dedicated offer-fee configurable vault ATA
    let fee_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_OFFER_FEE_VAULT]);
    assert_eq!(ctx.token_balance(&fee_vault, &ctx.usdc_mint), 10_000);
}

// ===========================================================================
// Redemption flow (create_redemption_request + status tracking)
// ===========================================================================

#[test]
fn test_create_redemption_request_and_read_back_state() {
    let mut ctx = setup_mint_offer();
    ctx.setup_redemption_offer();

    // User holds ONyc to redeem
    let user_onyc = 2_000_000_000u64;
    create_token_account(&mut ctx.svm, &ctx.onyc_mint, &ctx.user.pubkey(), user_onyc);

    // Integrator surface: read the redemption offer to get the counter
    let (redemption_offer_pda, _) = find_redemption_offer_pda(&ctx.onyc_mint, &ctx.usdc_mint);
    let ro_account = ctx.svm.get_account(&redemption_offer_pda).unwrap();
    let ro = RedemptionOffer::load(&ro_account.data).unwrap();
    assert_eq!(ro.request_counter, 0);
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
    create_token_account(&mut ctx.svm, &ctx.onyc_mint, &ctx.user.pubkey(), 1_000_000_000);

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
    create_token_account(&mut ctx.svm, &ctx.onyc_mint, &ctx.user.pubkey(), 1_000_000_000);

    let boss = ctx.payer.insecure_clone();
    let ix =
        build_set_redemption_offer_disabled_ix(&boss.pubkey(), &ctx.onyc_mint, &ctx.usdc_mint, true);
    ctx.send_ixs(&[ix], &[&boss]).unwrap();

    // Integrator can read the disabled flag off the account
    let (redemption_offer_pda, _) = find_redemption_offer_pda(&ctx.onyc_mint, &ctx.usdc_mint);
    let ro_account = ctx.svm.get_account(&redemption_offer_pda).unwrap();
    assert!(RedemptionOffer::load(&ro_account.data).unwrap().is_disabled());

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
