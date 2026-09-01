# OnRe Titan Integration

Titan integration primitives for OnRe's ONyc/USDG liquidity layer.

## Overview

This crate builds the v5 instructions Titan needs to route users in both
directions: USDG -> ONyc through the permissionless mint path and ONyc -> USDG
through the atomic proprietary-AMM sell path.

The local `TradingVenue` interface is an integration scaffold, not Titan's
published production trait. The final adapter still needs to connect Titan's
quote simulator/return-data boundary to the primitives in `prop_amm.rs`.

## Features

- **Bidirectional primitives**: USDG -> ONyc minting and atomic ONyc -> USDG sells
- **Time-based pricing**: APR-based discrete interval pricing model
- **Liquidity-aware sells**: Quotes reflect redemption-vault liquidity and Prop AMM controls
- **Protected execution**: Sells recompute on chain and enforce `minimum_out`
- **v5 permissionless minting**: Uses `take_offer_permissionless_v2`

## Key Components

### OnreVenue

Main struct implementing the Titan integration:

```rust
use onre_titan::OnreVenue;
use onre_titan::trading_venue::{FromAccount, QuoteRequest, SwapType};

// Load from offer account
let venue = OnreVenue::from_account(&offer_pubkey, &account)?;

// Update state from cache
venue.update_state(&cache).await?;

// Get quote
let quote = venue.quote(QuoteRequest {
    input_mint: usdg_mint,
    output_mint: onyc_mint,
    amount: 100_000_000, // 100 USDG
    swap_type: SwapType::ExactIn,
})?;

// Generate swap instruction
let ix = venue.generate_swap_instruction_v2(request, user_pubkey)?;
```

## Pricing Model

OnRe uses APR-based discrete interval pricing:

```
interval = floor((current_time - base_time) / price_fix_duration)
step_end_time = (interval + 1) * price_fix_duration
price = base_price * (1 + apr * step_end_time / SECONDS_IN_YEAR)
```

### APR Scale

- `APR_SCALE = 1,000,000` represents 100%
- 1% APR = 10,000
- 5% APR = 50,000
- 36.5% APR = 365,000

## Program Info

| Property | Value |
|----------|-------|
| Program ID | `onreuGhHHgVzMWSkj2oQDLDtvvGvoepBPkqyaubFcwe` |
| Legacy mint (deprecated) | `take_offer_permissionless` `[37, 190, 224, 77, 197, 39, 203, 230]` |
| Mint (v5) | `take_offer_permissionless_v2` `[250, 180, 68, 89, 124, 124, 31, 250]` |
| Atomic sell quote (v5) | `quote_swap_sell` `[198, 1, 48, 226, 172, 136, 51, 251]` |
| Atomic sell execution (v5) | `open_swap_sell` `[93, 206, 188, 72, 45, 138, 181, 71]` |
| Async redemption request (v5) | `create_redemption_request` `[201, 53, 181, 254, 115, 137, 70, 151]` |

## v5 Flows

### Buy ONyc (`take_offer_permissionless_v2`)

`OnreVenue::generate_swap_instruction_v2` builds the v5 permissionless take
with the full 32-account list: dedicated proceeds/fee configurable-vault PDAs,
redemption vault refill accounts, ONyc buffer accrual accounts, `market_stats`,
the circulating-supply excluded balance PDA and `state.main_offer`. The
instruction has no approval-message argument or Instructions sysvar. It uses
the offer's permissionless fee, not the regular-offer fee.

This is the supported transitional buy path. The protocol also exposes
`quote_swap_buy`/`open_swap_buy`; moving the adapter to that pair is follow-up
work so buy volume updates the same Prop AMM pressure tracker as sells.

### Sell ONyc atomically (`quote_swap_sell` + `open_swap_sell`)

The router simulates the quote instruction, validates the returned program id
and exact 152-byte Borsh payload, and binds it to the expected offer, mints and
input amount:

```rust
use onre_titan::prop_amm::*;

let quote_ix = build_quote_swap_sell_instruction(
    &onyc_mint, &usdg_mint, &usdg_token_program, amount,
)?;

// Titan supplies these values from transaction simulation.
let quote = parse_swap_sell_quote(
    &return_data_program, &return_data, &onyc_mint, &usdg_mint, amount,
)?;

let sell_ix = build_open_swap_sell_instruction(
    &user,
    &onyc_mint,
    &usdg_mint,
    &onyc_token_program,
    &usdg_token_program,
    &state_main_offer,
    &quote,
)?;
```

Never replace the quote's `minimum_out` with zero. `quoted_at` is
informational; execution recalculates price, liquidity and fees, and reverts if
the protected output cannot be met.

### Async redemption (separate from router swaps)

`create_redemption_request` locks ONyc and waits for protocol-side fulfillment.
It is not an atomic swap and must not be exposed as Titan's sell route:

```rust
use onre_titan::redemption::*;

// 1. Read the redemption offer (counter seeds the new request PDA)
let ro = RedemptionOffer::load(&redemption_offer_account.data)?;

// 2. Create the request (locks ONyc in the redemption vault)
let ix = build_create_redemption_request_instruction(
    &redeemer, &onyc_mint, &usdg_mint, amount, ro.request_counter,
);

// 3. Track it
let (request_pda, _) = find_redemption_request_pda(&redemption_offer_pda, ro.request_counter);
let status = redemption_request_status(account_data)?; // Pending | PartiallyFulfilled | Closed
```

The program closes the request account when it is fully fulfilled or
cancelled, so a missing account maps to `Closed`.

### Emergency states

`OnreVenue::status()` reports `Active | KillSwitchActive | OfferDisabled`;
`quote()` returns `not_enough_liquidity` for both emergency states. On-chain
failures classify via `classify_program_error` (6024 kill switch, 6112 offer
disabled, 6113 redemption offer disabled, 6025 permissionless not allowed)
so integrators can surface them as expected states instead of generic errors.

## Fees and liquidity

- The OnRe contract charges 20 bps for the RFQ liquidity layer.
- Titan adds 5 bps at the router/partner layer, for 25 bps total to the user.
- Partner-fee accounting is router-side; this crate does not invent an
  asset-specific Titan fee instruction.
- A deployed program is not the same as an active route. Routing must remain
  disabled until the ONyc/USDG pair is enabled and the redemption vault is
  funded. Planned initial liquidity is approximately 40 million USDG.

## Testing

Run tests:

```bash
cargo test
```

The LiteSVM integration tests (`tests/v5_integration.rs`) execute against the
locally built v5 program. Build it first:

```bash
cd ../onre-sol && git checkout master && anchor build
```

The binary is loaded from `../onre-sol/target/deploy/onreapp.so` by default;
override with `ONREAPP_SO_PATH`.

## Titan Requirements

The current scaffold covers these known Titan requirements:

- ✅ Zero-input quote handling (required for boundary search)
- ✅ Strict sell return-data origin, length and request binding
- ✅ Exact v5 instruction discriminators and account ordering
- ✅ Returns `QuoteResult` with `not_enough_liquidity` flag
- ✅ Generates permissionless-v2 buy and Prop AMM sell instructions

Still required before production enablement: wire Titan's real adapter to its
simulation/return-data API, run the same suite against the funded deployment,
and confirm the 5 bps partner accounting path end to end.

## Dependencies

- `solana-sdk = "2.2.1"`
- `spl-token = "7"`
- `spl-token-2022 = "9"`
- `async-trait = "0.1"`

## License

MIT
