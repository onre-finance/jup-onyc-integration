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
| Instruction | `take_offer_permissionless` |
| Discriminator | `[37, 190, 224, 77, 197, 39, 203, 230]` |

## Testing

Run tests:

```bash
cargo test
```

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
