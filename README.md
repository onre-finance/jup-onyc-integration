# OnRe Titan Integration

Titan TradingVenue integration for OnRe Protocol's ONyc minting mechanism.

## Overview

This crate implements Titan's `TradingVenue` trait for OnRe's permissionless ONyc token minting. It enables integration with Titan's unified routing layer, allowing users to mint ONyc tokens through Titan's aggregator.

## Features

- **Unidirectional**: Supports `token_in -> ONyc` minting only
- **Time-based pricing**: APR-based discrete interval pricing model
- **Zero-impact**: Trade size doesn't affect price within an interval
- **Permissionless**: Uses `take_offer_permissionless` instruction

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
    input_mint: usdc_mint,
    output_mint: onyc_mint,
    amount: 100_000_000, // 100 USDC
    swap_type: SwapType::ExactIn,
})?;

// Generate swap instruction
let ix = venue.generate_swap_instruction(request, user_pubkey)?;
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
| Mint (v1) | `take_offer_permissionless` `[37, 190, 224, 77, 197, 39, 203, 230]` |
| Mint (v5) | `take_offer_permissionless_v2` `[250, 180, 68, 89, 124, 124, 31, 250]` |
| Redemption (v5) | `create_redemption_request` `[201, 53, 181, 254, 115, 137, 70, 151]` |

## v5 Flows

### Mint (take_offer_permissionless_v2)

`OnreVenue::generate_swap_instruction_v2` builds the v5 permissionless take
with the full 33-account list: dedicated proceeds/fee configurable-vault PDAs,
redemption vault refill accounts, ONyc buffer accrual accounts, `market_stats`,
the circulating-supply excluded balance PDA and `state.main_offer`. The
approval message is always `None` (aggregator flow targets
`needs_approval = false` offers).

### Redemption (create + status tracking)

Fulfillment is protocol-side; the integrator surface is create + track:

```rust
use onre_titan::redemption::*;

// 1. Read the redemption offer (counter seeds the new request PDA)
let ro = RedemptionOffer::load(&redemption_offer_account.data)?;

// 2. Create the request (locks ONyc in the redemption vault)
let ix = build_create_redemption_request_instruction(
    &redeemer, &onyc_mint, &usdc_mint, amount, ro.request_counter,
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

## Testing

Run tests:

```bash
cargo test
```

The LiteSVM integration tests (`tests/v5_integration.rs`) execute against the
locally built v5 program. Build it first:

```bash
cd ../onre-sol && git checkout programV5 && anchor build
```

The binary is loaded from `../onre-sol/target/deploy/onreapp.so` by default;
override with `ONREAPP_SO_PATH`.

## Titan Requirements

This integration satisfies Titan's venue requirements:

- ✅ Zero-input quote handling (required for boundary search)
- ✅ No heap allocations in `quote()` 
- ✅ Defensive deserialization (no panics)
- ✅ Returns `QuoteResult` with `not_enough_liquidity` flag
- ✅ Generates valid swap `Instruction`

## Dependencies

- `solana-sdk = "2.2.1"`
- `spl-token = "7"`
- `spl-token-2022 = "9"`
- `async-trait = "0.1"`

## License

MIT
