# OnRe Jupiter Integration

Jupiter AMM integration for the OnRe Protocol, enabling users to mint ONyc tokens through Jupiter's aggregator.

## Overview

This crate implements Jupiter's `Amm` trait for the OnRe protocol's `take_offer_permissionless` instruction, allowing Jupiter to route swaps through OnRe's token minting mechanism.

### Key Characteristics

- **Unidirectional**: Only supports `token_in -> ONyc` direction
- **Time-based pricing**: Price grows based on APR and discrete intervals
- **No price impact**: Trade size doesn't affect price within an interval
- **Permissionless**: Uses `take_offer_permissionless` instruction

## Building

```bash
cargo build --release
```

## Testing

```bash
cargo test
```

## Project Structure

```
src/
├── lib.rs          # Main AMM implementation
├── constants.rs    # Program IDs, seeds, and constants
├── errors.rs       # Error types
├── pricing.rs      # Price calculation logic
└── state.rs        # Account deserialization
```

## Integration with Jupiter

Jupiter will:

1. Call `from_keyed_account()` with Offer account data to initialize the AMM
2. Call `get_accounts_to_update()` to know which accounts to fetch
3. Call `update()` with the fetched account data
4. Call `quote()` to get swap quotes
5. Call `get_swap_and_account_metas()` to build the swap transaction

## OnRe Protocol

| Property | Value |
|----------|-------|
| Program ID | `onreuGhHHgVzMWSkj2oQDLDtvvGvoepBPkqyaubFcwe` |
| Instruction | `take_offer_permissionless` |
| Network | Solana Mainnet |

## Pricing Model

OnRe uses discrete interval pricing with APR-based growth:

```
interval = floor((current_time - base_time) / price_fix_duration)
step_end_time = (interval + 1) * price_fix_duration
price = base_price * (1 + apr * step_end_time / SECONDS_IN_YEAR)
```

Where:
- `base_price` has 9 decimal precision (1.0 = 1,000,000,000)
- `apr` is scaled by 1,000,000 (1% APR = 1,000,000)
- `SECONDS_IN_YEAR` = 31,536,000