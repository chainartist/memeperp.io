# MemePerp.io

A Solana-based perpetual futures trading platform for memecoins, allowing leveraged trading on new pairs with dynamic liquidity management.

## Features

- Leveraged long/short positions on memecoin pairs
- Integration with pump.fun for price feeds
- Dynamic funding rate based on market imbalance
- Position size limits based on available liquidity
- Automatic liquidation system
- Fee collection mechanism

## Technical Details

### Market Parameters

- Minimum order size
- Maximum leverage
- Tick size
- Liquidation threshold
- Maintenance margin requirements
- Maximum position size
- Funding interval

### Funding Rate

The funding rate is calculated based on the imbalance between long and short positions:
- Updated every funding interval
- Rate is capped at ±0.1% per interval
- Longs pay shorts when longs > shorts
- Shorts pay longs when shorts > longs

### Liquidation

Positions are liquidated when:
- Margin ratio falls below maintenance requirement
- Price moves beyond liquidation threshold
- Insufficient margin to cover funding payments

### Position Size Limits

- Maximum position size per market
- Dynamic limits based on available liquidity
- Prevents market manipulation

## Development

### Prerequisites

- Solana Tool Suite
- Anchor Framework
- Node.js and npm

### Installation

```bash
npm install
```

### Building

```bash
anchor build
```

### Testing

```bash
anchor test
```

### Deployment

1. Update program ID in `lib.rs`
2. Build the program
3. Deploy to Solana network:
```bash
anchor deploy
```

## Security Considerations

- Price feed validation
- Overflow protection
- Liquidation thresholds
- Position size limits
- Fee calculations

## License

MIT
