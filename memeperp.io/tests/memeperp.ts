import * as anchor from "@project-serum/anchor";
import { Program } from "@project-serum/anchor";
import { Memeperp } from "../target/types/memeperp";
import { PublicKey, Keypair, SystemProgram } from "@solana/web3.js";
import { TOKEN_PROGRAM_ID, Token } from "@solana/spl-token";
import { assert } from "chai";

describe("memeperp", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Memeperp as Program<Memeperp>;
  
  let marketKeypair: Keypair;
  let marketVault: Keypair;
  let userTokenAccount: Keypair;
  let mockPriceFeed: Keypair;
  let mint: Token;
  
  // Constants
  const INITIAL_MINT_AMOUNT = new anchor.BN("1000000000000"); // 1M tokens
  const MIN_BASE_ORDER_SIZE = new anchor.BN("100000000"); // 100 tokens
  const TICK_SIZE = new anchor.BN("100"); // 0.01 USD
  const MAX_LEVERAGE = 20;
  const LIQUIDATION_THRESHOLD = 9500; // 95%
  const MAINTENANCE_MARGIN = 500; // 5%
  const MAX_POSITION_SIZE = new anchor.BN("100000000000"); // 100k tokens
  const FUNDING_INTERVAL = 3600; // 1 hour
  const MAX_PRICE_CHANGE_BPS = 1000; // 10%

  before(async () => {
    // Initialize market and token accounts
    marketKeypair = Keypair.generate();
    marketVault = Keypair.generate();
    userTokenAccount = Keypair.generate();
    mockPriceFeed = Keypair.generate();

    // Create mock price feed
    await provider.connection.confirmTransaction(
      await provider.connection.requestAirdrop(mockPriceFeed.publicKey, 1000000000)
    );
  });

  it("Initializes the market", async () => {
    await program.methods
      .initializeMarket(
        "DOGE/USD",
        new anchor.BN(MIN_BASE_ORDER_SIZE),
        new anchor.BN(TICK_SIZE),
        MAX_LEVERAGE,
        LIQUIDATION_THRESHOLD,
        MAINTENANCE_MARGIN
      )
      .accounts({
        market: marketKeypair.publicKey,
        authority: provider.wallet.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([marketKeypair])
      .rpc();

    const market = await program.account.market.fetch(marketKeypair.publicKey);
    assert.equal(market.name, "DOGE/USD");
    assert.equal(market.minBaseOrderSize.toNumber(), MIN_BASE_ORDER_SIZE);
    assert.equal(market.maxLeverage, MAX_LEVERAGE);
  });

  it("Places a long position", async () => {
    const size = new anchor.BN(1000);
    const price = new anchor.BN(100);
    const leverage = 5;

    await program.methods
      .placeOrder(
        { long: {} },
        size,
        price,
        leverage
      )
      .accounts({
        market: marketKeypair.publicKey,
        user: provider.wallet.publicKey,
        userTokenAccount: userTokenAccount.publicKey,
        marketVault: marketVault.publicKey,
        priceFeed: mockPriceFeed.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc();

    const market = await program.account.market.fetch(marketKeypair.publicKey);
    assert.equal(market.longPositions.length, 1);
  });

  it("Places a short position", async () => {
    const size = new anchor.BN(500);
    const price = new anchor.BN(100);
    const leverage = 3;

    await program.methods
      .placeOrder(
        { short: {} },
        size,
        price,
        leverage
      )
      .accounts({
        market: marketKeypair.publicKey,
        user: provider.wallet.publicKey,
        userTokenAccount: userTokenAccount.publicKey,
        marketVault: marketVault.publicKey,
        priceFeed: mockPriceFeed.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc();

    const market = await program.account.market.fetch(marketKeypair.publicKey);
    assert.equal(market.shortPositions.length, 1);
  });

  it("Liquidates an underwater position", async () => {
    // Set up a position that will be underwater
    const size = new anchor.BN(1000);
    const price = new anchor.BN(100);
    const leverage = 10;

    await program.methods
      .placeOrder(
        { long: {} },
        size,
        price,
        leverage
      )
      .accounts({
        market: marketKeypair.publicKey,
        user: provider.wallet.publicKey,
        userTokenAccount: userTokenAccount.publicKey,
        marketVault: marketVault.publicKey,
        priceFeed: mockPriceFeed.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc();

    // Liquidate the position
    await program.methods
      .liquidatePosition(new anchor.BN(0), { long: {} })
      .accounts({
        market: marketKeypair.publicKey,
        userTokenAccount: userTokenAccount.publicKey,
        marketVault: marketVault.publicKey,
        priceFeed: mockPriceFeed.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc();

    const market = await program.account.market.fetch(marketKeypair.publicKey);
    assert.equal(market.longPositions.length, 1); // The first position remains
  });
});
