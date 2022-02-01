token-bonding-curve is forked from anchor-token-swap (which is a fork of spl token-swap with anchor added) with a custom LinearPriceCurve type added in src/curve/linear_price.rs. It models a curve where the price of the output `token b` increases at a linear rate as more collateral `token a` has been swapped in. See docs in linear_price.rs for more calculation details and cavets. 

e.g. a curve with formula `a = 3b + 2` – where a is the price of a single bonded `token b` (denominated in amount of `token a`) and b is the amount of `token b` that's been swapped out of this curve – starts at a price of `2 token A in required to get 1 token B out` when 0 `token b` has been exchanged and increases by `3 token A to get 1 token B out` for every 1 `token b` that's swapped out

Under the hood it uses the integral of the price formula to calculate the amount of `token a` locked in the curve and uses that to determine the spot price and the amount of destination token to emit 

Pool tokens and deposits/withdrawals of pool tokens are intentionally disabled so that liquidity can't be added/removed from the swap outside of the `swap` instruction. If more liquidity is required, a second curve can be initialized with the same slope and an appropriately set start price (e.g. the end price of the previous curve). Fees are also disabled (at the instruction level, see lib.rs:initialize_linear_price).

See https://github.com/rally-dfs/anchor-token-swap/blob/main/README.md and https://github.com/solana-labs/solana-program-library/tree/master/token-swap where this was forked from too