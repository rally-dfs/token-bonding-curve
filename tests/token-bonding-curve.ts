import * as anchor from '@project-serum/anchor';
import assert from "assert";
import { PublicKey } from '@solana/web3.js';
import { AccountLayout, MintLayout, Token, TOKEN_PROGRAM_ID } from '@solana/spl-token';



const TOKEN_PROGRAM_PUBKEY = new anchor.web3.PublicKey(TOKEN_PROGRAM_ID);
const SWAP_ACCOUNT_SPACE = 324;

const generateNewSignerAccount = async (provider: anchor.Provider) => {
  return generateNewGenericAccount(provider, provider.wallet.publicKey, 8 + 8, anchor.web3.SystemProgram.programId, 10);
}

const generateNewGenericAccount = async (
  provider: anchor.Provider, fromPubkey: PublicKey, space: number, programId: PublicKey, extraLamports: number
) => {
  const newAccount = anchor.web3.Keypair.generate();

  // Create account transaction.
  const tx = new anchor.web3.Transaction();
  tx.add(
    anchor.web3.SystemProgram.createAccount({
      fromPubkey: fromPubkey,
      newAccountPubkey: newAccount.publicKey,
      space,
      lamports: await provider.connection.getMinimumBalanceForRentExemption(
        space
      ) + extraLamports,
      programId
    })
  );

  await provider.send(tx, [newAccount]);

  return newAccount;
}

const generateTokenMint = async (provider: anchor.Provider, authority: PublicKey) => {
  const mint = anchor.web3.Keypair.generate();

  const instructions = [
    //create account with mint account layout
    anchor.web3.SystemProgram.createAccount({
      fromPubkey: provider.wallet.publicKey,
      newAccountPubkey: mint.publicKey,
      space: MintLayout.span,
      lamports: await provider.connection.getMinimumBalanceForRentExemption(MintLayout.span),
      programId: TOKEN_PROGRAM_ID,
    }),

    // initialize mint account
    Token.createInitMintInstruction(
      // program id
      TOKEN_PROGRAM_ID,
      // mint pub key
      mint.publicKey,
      // decimals
      9,
      // mint authority
      authority,
      // freeze authority - note this must be null for token-swap pool
      null
    ),
  ]

  const tx = new anchor.web3.Transaction();
  tx.add(...instructions);

  await provider.send(tx, [mint]);

  return mint;
}

const generateTokenAccount = async (provider: anchor.Provider, mint: anchor.web3.Keypair, owner: PublicKey) => {
  const tokenAccount = anchor.web3.Keypair.generate();

  const instructions = [
    //create account with token account layout
    anchor.web3.SystemProgram.createAccount({
      fromPubkey: provider.wallet.publicKey,
      newAccountPubkey: tokenAccount.publicKey,
      space: AccountLayout.span,
      lamports: await provider.connection.getMinimumBalanceForRentExemption(AccountLayout.span),
      programId: TOKEN_PROGRAM_ID,
    }),

    //initialize token account for specified mint
    Token.createInitAccountInstruction(
      // program id
      TOKEN_PROGRAM_ID,
      // mint pub key
      mint.publicKey,
      // token account pub key
      tokenAccount.publicKey,
      // owner
      owner,
    ),
  ]

  const tx = new anchor.web3.Transaction();
  tx.add(...instructions);

  await provider.send(tx, [tokenAccount]);

  return tokenAccount;
}

const mintToAccount = async (
  provider: anchor.Provider, authority: anchor.web3.Keypair, mint: anchor.web3.Keypair, tokenAccount: PublicKey,
  amount: number
) => {
  const instructions = [
    Token.createMintToInstruction(TOKEN_PROGRAM_ID, mint.publicKey, tokenAccount, authority.publicKey, [], amount)
  ]

  const tx = new anchor.web3.Transaction();
  tx.add(...instructions);

  await provider.send(tx, [authority]);
}

describe('token-bonding-curve', () => {

  // Configure the client to use the local cluster.
  const provider = anchor.Provider.env();
  anchor.setProvider(provider);

  it('should perform constant price swap!', async () => {

    const program = anchor.workspace.TokenBondingCurve;

    // TODO: doing these in separate txns is really slow, could probably be optimized

    // owner of token A and token B mint, unrelated to swapAuthority
    const aTokenMintAuthority = await generateNewSignerAccount(provider);
    const bTokenMintAuthority = await generateNewSignerAccount(provider);

    const aTokenMint = await generateTokenMint(provider, aTokenMintAuthority.publicKey);
    const bTokenMint = await generateTokenMint(provider, bTokenMintAuthority.publicKey);

    ///   0. `[writable, signer]` New Token-swap to create.
    const tokenSwap = await generateNewGenericAccount(provider, provider.wallet.publicKey, SWAP_ACCOUNT_SPACE, program.programId, 0);
    ///   1. `[]` swap authority derived from `create_program_address(&[Token-swap account])`
    // corresponds to processor.rs Pubkey::find_program_address(&[&swap_info.key.to_bytes()], program_id);
    const swapAuthority = (await anchor.web3.PublicKey.findProgramAddress([tokenSwap.publicKey.toBuffer()], program.programId))[0];

    ///   2. `[]` token_a Account. Must be non zero, owned by swap authority.
    ///   3. `[]` token_b Account. Must be non zero, owned by swap authority.
    const aTokenSwapAccount = await generateTokenAccount(provider, aTokenMint, swapAuthority);
    await mintToAccount(provider, aTokenMintAuthority, aTokenMint, aTokenSwapAccount.publicKey, 1000 * 10 ** 8);
    const bTokenSwapAccount = await generateTokenAccount(provider, bTokenMint, swapAuthority);
    await mintToAccount(provider, bTokenMintAuthority, bTokenMint, bTokenSwapAccount.publicKey, 2000 * 10 ** 8);

    let aToken = new Token(provider.connection, aTokenMint.publicKey, TOKEN_PROGRAM_ID, aTokenMintAuthority);
    let bToken = new Token(provider.connection, bTokenMint.publicKey, TOKEN_PROGRAM_ID, bTokenMintAuthority);

    ///   4. `[writable]` Pool Token Mint. Must be empty, owned by swap authority.
    const poolTokenMint = await generateTokenMint(provider, swapAuthority);

    let poolToken = new Token(provider.connection, poolTokenMint.publicKey, TOKEN_PROGRAM_ID, tokenSwap);

    ///   5. `[]` Pool Token Account to deposit trading and withdraw fees.
    ///   Must be empty, not owned by swap authority
    const feeAuthority = await generateNewSignerAccount(provider);
    const feeTokenAccount = await generateTokenAccount(provider, poolTokenMint, feeAuthority.publicKey);

    ///   6. `[writable]` Pool Token Account to deposit the initial pool token
    ///   supply.  Must be empty, not owned by swap authority.
    const destinationAuthority = await generateNewSignerAccount(provider);
    const destinationTokenAccount = await generateTokenAccount(provider, poolTokenMint, destinationAuthority.publicKey);

    let trade_fee_numerator = new anchor.BN(2);
    let trade_fee_denominator = new anchor.BN(100);
    let owner_trade_fee_numerator = new anchor.BN(3);
    let owner_trade_fee_denominator = new anchor.BN(100);
    let owner_withdraw_fee_numerator = new anchor.BN(7);
    let owner_withdraw_fee_denominator = new anchor.BN(100);
    let host_fee_numerator = new anchor.BN(11);
    let host_fee_denominator = new anchor.BN(100);

    let fees = {
      trade_fee_numerator,
      trade_fee_denominator,
      owner_trade_fee_numerator,
      owner_trade_fee_denominator,
      owner_withdraw_fee_numerator,
      owner_withdraw_fee_denominator,
      host_fee_numerator,
      host_fee_denominator,
    };

    let token_b_price = new anchor.BN(5);

    const tx = await program.rpc.initializeConstantPrice(
      // TODO: not sure why passing fees in here doesn't work, need to look into it
      trade_fee_numerator,
      trade_fee_denominator,
      owner_trade_fee_numerator,
      owner_trade_fee_denominator,
      owner_withdraw_fee_numerator,
      owner_withdraw_fee_denominator,
      host_fee_numerator,
      host_fee_denominator,
      token_b_price,
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          tokenA: aTokenSwapAccount.publicKey,
          tokenB: bTokenSwapAccount.publicKey,
          pool: poolTokenMint.publicKey,
          fee: feeTokenAccount.publicKey,
          destination: destinationTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [tokenSwap],
      });

    console.log("Your transaction signature", tx);

    // fee token account starts at 0
    assert.strictEqual(
      (await poolToken.getAccountInfo(feeTokenAccount.publicKey)).amount.toString(),
      "0");
    // destination token starts at 10 (see CurveCalculator::INITIAL_SWAP_POOL_AMOUNT)
    assert.strictEqual(
      (await poolToken.getAccountInfo(destinationTokenAccount.publicKey)).amount.toString(),
      "10.00000000".replace(".", ""));

    const swapUser = await generateNewSignerAccount(provider);

    const aTokenUserAccount = await generateTokenAccount(provider, aTokenMint, swapUser.publicKey);
    await mintToAccount(provider, aTokenMintAuthority, aTokenMint, aTokenUserAccount.publicKey, 30 * 10 ** 8);
    const bTokenUserAccount = await generateTokenAccount(provider, bTokenMint, swapUser.publicKey);
    await mintToAccount(provider, bTokenMintAuthority, bTokenMint, bTokenUserAccount.publicKey, 200 * 10 ** 8);

    let amount_in = new anchor.BN(20 * 10 ** 8);
    let minimum_amount_out = new anchor.BN(0);

    const swapTx = await program.rpc.swap(
      amount_in,
      minimum_amount_out,
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: aTokenUserAccount.publicKey,
          swapSource: aTokenSwapAccount.publicKey,
          swapDestination: bTokenSwapAccount.publicKey,
          destination: bTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    )

    console.log("Your transaction signature", swapTx);

    // user swaps 20 token A, balance 30 -> 10
    assert.strictEqual(
      (await aToken.getAccountInfo(aTokenUserAccount.publicKey)).amount.toString(),
      "10.00000000".replace(".", ""));
    // swap's A balance goes from 1000 -> 1020
    assert.strictEqual(
      (await aToken.getAccountInfo(aTokenSwapAccount.publicKey)).amount.toString(),
      "1020.00000000".replace(".", ""));
    // user gets 20/5 B tokens back, minus fees 200 -> 203.8
    // TODO: just got +3.8 by running the program, should probably verify the fee math
    assert.strictEqual(
      (await bToken.getAccountInfo(bTokenUserAccount.publicKey)).amount.toString(),
      "203.80000000".replace(".", ""));
    // swap's B balance goes from 2000 -> 1996.2
    assert.strictEqual(
      (await bToken.getAccountInfo(bTokenSwapAccount.publicKey)).amount.toString(),
      "1996.20000000".replace(".", ""));
  });

  const generateTestLinearSwapAccounts = async (programId: PublicKey, cTokenInitialSupply: number) => {

    // TODO: doing these in separate txns is really slow, could probably be optimized

    // owner of token A and token B mint, unrelated to swapAuthority
    const rTokenMintAuthority = await generateNewSignerAccount(provider);
    const cTokenMintAuthority = await generateNewSignerAccount(provider);

    const rTokenMint = await generateTokenMint(provider, rTokenMintAuthority.publicKey);
    const cTokenMint = await generateTokenMint(provider, cTokenMintAuthority.publicKey);

    ///   0. `[writable, signer]` New Token-swap to create.
    const tokenSwap = await generateNewGenericAccount(provider, provider.wallet.publicKey, SWAP_ACCOUNT_SPACE, programId, 0);
    ///   1. `[]` swap authority derived from `create_program_address(&[Token-swap account])`
    // corresponds to processor.rs Pubkey::find_program_address(&[&swap_info.key.to_bytes()], program_id);
    const swapAuthority = (await anchor.web3.PublicKey.findProgramAddress([tokenSwap.publicKey.toBuffer()], programId))[0];

    ///   2. `[]` token_a Account. Must be non zero, owned by swap authority.
    ///   3. `[]` token_b Account. Must be non zero, owned by swap authority.
    const rTokenSwapAccount = await generateTokenAccount(provider, rTokenMint, swapAuthority);
    // note we must start with 0 RLY so no need to mint any token A here
    const cTokenSwapAccount = await generateTokenAccount(provider, cTokenMint, swapAuthority);
    await mintToAccount(provider, cTokenMintAuthority, cTokenMint, cTokenSwapAccount.publicKey, cTokenInitialSupply);

    const rToken = new Token(provider.connection, rTokenMint.publicKey, TOKEN_PROGRAM_ID, rTokenMintAuthority);
    const cToken = new Token(provider.connection, cTokenMint.publicKey, TOKEN_PROGRAM_ID, cTokenMintAuthority);

    ///   4. `[writable]` Pool Token Mint. Must be empty, owned by swap authority.
    const poolTokenMint = await generateTokenMint(provider, swapAuthority);

    const poolToken = new Token(provider.connection, poolTokenMint.publicKey, TOKEN_PROGRAM_ID, tokenSwap);

    ///   5. `[]` Pool Token Account to deposit trading and withdraw fees.
    ///   Must be empty, not owned by swap authority
    const feeAuthority = await generateNewSignerAccount(provider);
    const feeTokenAccount = await generateTokenAccount(provider, poolTokenMint, feeAuthority.publicKey);

    ///   6. `[writable]` Pool Token Account to deposit the initial pool token
    ///   supply.  Must be empty, not owned by swap authority.
    const destinationAuthority = await generateNewSignerAccount(provider);
    const destinationTokenAccount = await generateTokenAccount(provider, poolTokenMint, destinationAuthority.publicKey);

    return {
      rTokenMintAuthority,
      cTokenMintAuthority,
      rTokenMint,
      cTokenMint,
      tokenSwap,
      swapAuthority,
      rTokenSwapAccount,
      cTokenSwapAccount,
      rToken,
      cToken,
      poolTokenMint,
      poolToken,
      feeAuthority,
      feeTokenAccount,
      destinationAuthority,
      destinationTokenAccount,
    };
  };

  it('should initialize linear price swap!', async () => {
    const program = anchor.workspace.TokenBondingCurve;

    const {
      rTokenMintAuthority,
      cTokenMintAuthority,
      rTokenMint,
      cTokenMint,
      tokenSwap,
      swapAuthority,
      rTokenSwapAccount,
      cTokenSwapAccount,
      rToken,
      cToken,
      poolTokenMint,
      poolToken,
      feeAuthority,
      feeTokenAccount,
      destinationAuthority,
      destinationTokenAccount,
    } = await generateTestLinearSwapAccounts(program.programId, 500 * 10 ** 8);

    // example curve - 0.5 slope (i.e. price increases by "1 base RLY per base CC" for every 2 display CC AKA 2e8 CC), starting price of 50 RLY at 300 (display) CC
    let slope_numerator = new anchor.BN(1);
    let slope_denominator = new anchor.BN(200000000);
    let r0_numerator = new anchor.BN(150);  // since R and C both have 8 decimals, we don't need to do any scaling here (starts at 50 base RLY price for every 1 base CC)
    let r0_denominator = new anchor.BN(3);  // not reducing to test out division

    const tx = await program.rpc.initializeLinearPrice(
      slope_numerator,
      slope_denominator,
      r0_numerator,
      r0_denominator,
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          tokenA: rTokenSwapAccount.publicKey,
          tokenB: cTokenSwapAccount.publicKey,
          pool: poolTokenMint.publicKey,
          fee: feeTokenAccount.publicKey,
          destination: destinationTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [tokenSwap],
      });

    console.log("Your transaction signature", tx);

    // fee token account starts at 0
    assert.strictEqual(
      (await poolToken.getAccountInfo(feeTokenAccount.publicKey)).amount.toString(),
      "0".replace(".", ""));
    // destination token starts at 10 (see CurveCalculator::INITIAL_SWAP_POOL_AMOUNT)
    assert.strictEqual(
      (await poolToken.getAccountInfo(destinationTokenAccount.publicKey)).amount.toString(),
      "10.00000000".replace(".", ""));

    const swapUser = await generateNewSignerAccount(provider);

    // start with 10K RLY and 200 CC
    const rTokenUserAccount = await generateTokenAccount(provider, rTokenMint, swapUser.publicKey);
    await mintToAccount(provider, rTokenMintAuthority, rTokenMint, rTokenUserAccount.publicKey, 10000 * 10 ** 8);
    const cTokenUserAccount = await generateTokenAccount(provider, cTokenMint, swapUser.publicKey);
    await mintToAccount(provider, cTokenMintAuthority, cTokenMint, cTokenUserAccount.publicKey, 200 * 10 ** 8);

    // put in 2400 RLY, should get out 40 CC
    let swapTx = await program.rpc.swap(
      new anchor.BN("240000000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: rTokenUserAccount.publicKey,
          swapSource: rTokenSwapAccount.publicKey,
          swapDestination: cTokenSwapAccount.publicKey,
          destination: cTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    )

    console.log("Your transaction signature", swapTx);

    // user RLY goes from 10K -> 7600
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount.publicKey)).amount.toString(),
      "7600.00000000".replace(".", ""));
    // swap's RLY balance goes from 0 -> 2400
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      "2400.00000000".replace(".", ""));
    // user CC goes from 200 -> 240
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount.publicKey)).amount.toString(),
      "240.00000000".replace(".", ""));
    // swap's CC balance goes from 500 -> 460
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "460.00000000".replace(".", ""));

    // have another user swap 1500 RLY, get 20 CC out now
    const swapUser2 = await generateNewSignerAccount(provider);

    const rTokenUserAccount2 = await generateTokenAccount(provider, rTokenMint, swapUser2.publicKey);
    await mintToAccount(provider, rTokenMintAuthority, rTokenMint, rTokenUserAccount2.publicKey, 4000 * 10 ** 8);
    const cTokenUserAccount2 = await generateTokenAccount(provider, cTokenMint, swapUser2.publicKey);
    await mintToAccount(provider, cTokenMintAuthority, cTokenMint, cTokenUserAccount2.publicKey, 100 * 10 ** 8);

    swapTx = await program.rpc.swap(
      new anchor.BN("150000000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser2.publicKey,
          source: rTokenUserAccount2.publicKey,
          swapSource: rTokenSwapAccount.publicKey,
          swapDestination: cTokenSwapAccount.publicKey,
          destination: cTokenUserAccount2.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser2]
      },
    )

    console.log("Your transaction signature", swapTx);

    // user2's RLY goes from 4000 -> 2500
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount2.publicKey)).amount.toString(),
      "2500.00000000".replace(".", ""));
    // swap's RLY balance goes from 2400 -> 3900
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      "3900.00000000".replace(".", ""));
    // user2's CC goes from 100 -> 120
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount2.publicKey)).amount.toString(),
      "120.00000000".replace(".", ""));
    // swap's CC balance goes from 460 -> 440
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "440.00000000".replace(".", ""));

    // first user swaps back 30 CC -> 2175 RLY
    swapTx = await program.rpc.swap(
      new anchor.BN("3000000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: cTokenUserAccount.publicKey,
          swapSource: cTokenSwapAccount.publicKey,
          swapDestination: rTokenSwapAccount.publicKey,
          destination: rTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    )

    console.log("Your transaction signature", swapTx);

    // user RLY goes from 7600 -> 9775
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount.publicKey)).amount.toString(),
      "9775.00000000".replace(".", ""));
    // swap's RLY balance goes from 3900 -> 1725
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      "1725.00000000".replace(".", ""));
    // user CC goes from 240 -> 210
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount.publicKey)).amount.toString(),
      "210.00000000".replace(".", ""));
    // swap's CC balance goes from 440 -> 470
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "470.00000000".replace(".", ""));

    // second user puts in 50CC, should return all the RLY but not overcharge the user (only should take 30CC)
    swapTx = await program.rpc.swap(
      new anchor.BN("5000000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser2.publicKey,
          source: cTokenUserAccount2.publicKey,
          swapSource: cTokenSwapAccount.publicKey,
          swapDestination: rTokenSwapAccount.publicKey,
          destination: rTokenUserAccount2.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser2]
      },
    )

    console.log("Your transaction signature", swapTx);

    // user RLY goes from 2500 -> 4225
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount2.publicKey)).amount.toString(),
      "4225.00000000".replace(".", ""));
    // swap's RLY balance goes from 1725 -> 0
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      "0".replace(".", ""));
    // user CC goes from 120 -> 90 (should take 30 and not the whole 50)
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount2.publicKey)).amount.toString(),
      "90.00000000".replace(".", ""));
    // swap's CC balance goes from 470 -> 500
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "500.00000000".replace(".", ""));

    // have user 1 put in 100K RLY, should just get all of the 500 CC pool (and only charge 87500 RLY)
    await mintToAccount(provider, rTokenMintAuthority, rTokenMint, rTokenUserAccount.publicKey, 100000 * 10 ** 8);

    swapTx = await program.rpc.swap(
      new anchor.BN("10000000000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: rTokenUserAccount.publicKey,
          swapSource: rTokenSwapAccount.publicKey,
          swapDestination: cTokenSwapAccount.publicKey,
          destination: cTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    )

    console.log("Your transaction signature", swapTx);

    // user RLY goes from 109775 -> 22275
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount.publicKey)).amount.toString(),
      "22275.00000000".replace(".", ""));
    // swap's RLY balance goes from 0 -> 87500K
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      "87500.00000000".replace(".", ""));
    // user CC goes from 210 -> 710
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount.publicKey)).amount.toString(),
      "710.00000000".replace(".", ""));
    // swap's CC balance goes from 500 -> 0
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "0".replace(".", ""));
  });

  it('should fail invalid linear price swaps!', async () => {

    const program = anchor.workspace.TokenBondingCurve;

    const {
      rTokenMintAuthority,
      cTokenMintAuthority,
      rTokenMint,
      cTokenMint,
      tokenSwap,
      swapAuthority,
      rTokenSwapAccount,
      cTokenSwapAccount,
      rToken,
      cToken,
      poolTokenMint,
      poolToken,
      feeAuthority,
      feeTokenAccount,
      destinationAuthority,
      destinationTokenAccount,
    } = await generateTestLinearSwapAccounts(program.programId, 0); // note we mint 0 initial tokens here

    // example curve - 0.5 slope (i.e. price increases by "1 base RLY per base CC" for every 2 display CC AKA 2e8 CC), starting price of 50 RLY at 300 (display) CC
    let slope_numerator = new anchor.BN(1);
    let slope_denominator = new anchor.BN(200000000);
    let r0_numerator = new anchor.BN(150);  // since R and C both have 8 decimals, we don't need to do any scaling here (starts at 50 base RLY price for every 1 base CC)
    let r0_denominator = new anchor.BN(3);  // not reducing to test out division

    // zero token B on init should fail 
    await assert.rejects(program.rpc.initializeLinearPrice(
      slope_numerator,
      slope_denominator,
      r0_numerator,
      r0_denominator,
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          tokenA: rTokenSwapAccount.publicKey,
          tokenB: cTokenSwapAccount.publicKey,
          pool: poolTokenMint.publicKey,
          fee: feeTokenAccount.publicKey,
          destination: destinationTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [tokenSwap],
      }));

    // non zero collateral token not allowed
    await mintToAccount(provider, cTokenMintAuthority, cTokenMint, cTokenSwapAccount.publicKey, 1);
    await mintToAccount(provider, rTokenMintAuthority, rTokenMint, rTokenSwapAccount.publicKey, 1);

    await assert.rejects(program.rpc.initializeLinearPrice(
      slope_numerator,
      slope_denominator,
      r0_numerator,
      r0_denominator,
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          tokenA: rTokenSwapAccount.publicKey,
          tokenB: cTokenSwapAccount.publicKey,
          pool: poolTokenMint.publicKey,
          fee: feeTokenAccount.publicKey,
          destination: destinationTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [tokenSwap],
      }));
  });

  it('should disallow linear price swaps deposits/withdrawals!', async () => {
    const program = anchor.workspace.TokenBondingCurve;

    const {
      rTokenMintAuthority,
      cTokenMintAuthority,
      rTokenMint,
      cTokenMint,
      tokenSwap,
      swapAuthority,
      rTokenSwapAccount,
      cTokenSwapAccount,
      rToken,
      cToken,
      poolTokenMint,
      poolToken,
      feeAuthority,
      feeTokenAccount,
      destinationAuthority,
      destinationTokenAccount,
    } = await generateTestLinearSwapAccounts(program.programId, 500 * 10 ** 8);

    // example curve - 0.5 slope (i.e. price increases by "1 base RLY per base CC" for every 2 display CC AKA 2e8 CC), starting price of 50 RLY at 300 (display) CC
    let slope_numerator = new anchor.BN(1);
    let slope_denominator = new anchor.BN(200000000);
    let r0_numerator = new anchor.BN(150);  // since R and C both have 8 decimals, we don't need to do any scaling here (starts at 50 base RLY price for every 1 base CC)
    let r0_denominator = new anchor.BN(3);  // not reducing to test out division

    const tx = await program.rpc.initializeLinearPrice(
      slope_numerator,
      slope_denominator,
      r0_numerator,
      r0_denominator,
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          tokenA: rTokenSwapAccount.publicKey,
          tokenB: cTokenSwapAccount.publicKey,
          pool: poolTokenMint.publicKey,
          fee: feeTokenAccount.publicKey,
          destination: destinationTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [tokenSwap],
      });

    // make sure deposits are disabled (create a new A/B token holder to try and deposit)
    const testUser = await generateNewSignerAccount(provider);

    const rTokenUserAccount = await generateTokenAccount(provider, rTokenMint, testUser.publicKey);
    await mintToAccount(provider, rTokenMintAuthority, rTokenMint, rTokenUserAccount.publicKey, 100 * 10 ** 8);
    const cTokenUserAccount = await generateTokenAccount(provider, cTokenMint, testUser.publicKey);
    await mintToAccount(provider, cTokenMintAuthority, cTokenMint, cTokenUserAccount.publicKey, 200 * 10 ** 8);

    const poolTokenUserAccount = await generateTokenAccount(provider, poolTokenMint, testUser.publicKey);

    // try to put in (at most) 100 A tokens/200 B tokens and get 10 pool tokens out
    let poolTokenAmount = new anchor.BN(10 * 10 ** 8);
    let maximumTokenAAmount = new anchor.BN(100 * 10 ** 8);
    let maximumTokenBAmount = new anchor.BN(200 * 10 ** 8);

    await assert.rejects(program.rpc.depositAllTokenTypes(
      poolTokenAmount,
      maximumTokenAAmount,
      maximumTokenBAmount,
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: testUser.publicKey,
          sourceA: rTokenUserAccount.publicKey,
          sourceB: cTokenUserAccount.publicKey,
          tokenA: rTokenSwapAccount.publicKey,
          tokenB: cTokenSwapAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          destination: poolTokenUserAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [testUser],
      }));

    // make sure deposit single single also doesn't work

    // try to put in exactly 100 B tokens and get (at least) 10 pool tokens out
    let sourceTokenAmount = new anchor.BN(100 * 10 ** 8);
    let minimumPoolTokenAmount = new anchor.BN(10 * 10 ** 8);

    await assert.rejects(program.rpc.depositSingleTokenTypeExactAmountIn(
      sourceTokenAmount,
      minimumPoolTokenAmount,
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: testUser.publicKey,
          // note we're using token B here
          sourceToken: cTokenUserAccount.publicKey,
          swapTokenA: rTokenSwapAccount.publicKey,
          swapTokenB: cTokenSwapAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          destination: poolTokenUserAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [testUser],
      }));

    // make sure withdrawals are disabled (reuse destinationTokenAccount since it holds all the pool tokens)
    const destinationRTokenAccount = await generateTokenAccount(provider, rTokenMint, destinationAuthority.publicKey);
    const destinationCTokenAccount = await generateTokenAccount(provider, cTokenMint, destinationAuthority.publicKey);

    // try to put in 10 pool tokens and get (at least) 100 B tokens out
    poolTokenAmount = new anchor.BN(10 * 10 ** 8);
    let minimumTokenAAmount = new anchor.BN(0); // this should be 0 since there's no A in the swap yet
    let minimumTokenBAmount = new anchor.BN(100 * 10 ** 8);

    await assert.rejects(program.rpc.withdrawAllTokenTypes(
      poolTokenAmount,
      minimumTokenAAmount,
      minimumTokenBAmount,
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: destinationAuthority.publicKey,
          poolMint: poolTokenMint.publicKey,
          source: destinationTokenAccount.publicKey,
          swapTokenA: rTokenSwapAccount.publicKey,
          swapTokenB: cTokenSwapAccount.publicKey,
          destinationTokenA: destinationRTokenAccount.publicKey,
          destinationTokenB: destinationCTokenAccount.publicKey,
          feeAccount: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [destinationAuthority],
      }));

    // also check single token withdrawals
    // try to put in (at most) 10 pool tokens and get exactly 10 B tokens out
    let destinationTokenAmount = new anchor.BN(100 * 10 ** 8);
    let maximumPoolTokenAmount = new anchor.BN(10 * 10 ** 8);

    await assert.rejects(program.rpc.withdrawSingleTokenTypeExactAmountOut(
      destinationTokenAmount,
      maximumPoolTokenAmount,
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: destinationAuthority.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolTokenSource: destinationTokenAccount.publicKey,
          swapTokenA: rTokenSwapAccount.publicKey,
          swapTokenB: cTokenSwapAccount.publicKey,
          // note we're using B as the token here (swap has 0 A token right now anyway)
          destination: destinationCTokenAccount.publicKey,
          poolFeeAccount: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [destinationAuthority],
      }));
  });

  it('should handle taki low slope!', async () => {
    const program = anchor.workspace.TokenBondingCurve;

    const {
      rTokenMintAuthority,
      cTokenMintAuthority,
      rTokenMint,
      cTokenMint,
      tokenSwap,
      swapAuthority,
      rTokenSwapAccount,
      cTokenSwapAccount,
      rToken,
      cToken,
      poolTokenMint,
      poolToken,
      feeAuthority,
      feeTokenAccount,
      destinationAuthority,
      destinationTokenAccount,
    } = await generateTestLinearSwapAccounts(program.programId, 5 * 10 ** 9);

    // TAKI curve with huge denominator
    let slope_numerator = new anchor.BN(37);
    let slope_denominator = new anchor.BN("1400000000000000000");
    let r0_numerator = new anchor.BN(7);
    let r0_denominator = new anchor.BN(2);

    const tx = await program.rpc.initializeLinearPrice(
      slope_numerator,
      slope_denominator,
      r0_numerator,
      r0_denominator,
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          tokenA: rTokenSwapAccount.publicKey,
          tokenB: cTokenSwapAccount.publicKey,
          pool: poolTokenMint.publicKey,
          fee: feeTokenAccount.publicKey,
          destination: destinationTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [tokenSwap],
      });

    console.log("Your transaction signature", tx);

    // fee token account starts at 0
    assert.strictEqual(
      (await poolToken.getAccountInfo(feeTokenAccount.publicKey)).amount.toString(),
      "0".replace(".", ""));
    // destination token starts at 10 (see CurveCalculator::INITIAL_SWAP_POOL_AMOUNT)
    assert.strictEqual(
      (await poolToken.getAccountInfo(destinationTokenAccount.publicKey)).amount.toString(),
      "10.00000000".replace(".", ""));

    const swapUser = await generateNewSignerAccount(provider);

    // user starts with 10 RLY and 1 TAKI
    const rTokenUserAccount = await generateTokenAccount(provider, rTokenMint, swapUser.publicKey);
    await mintToAccount(provider, rTokenMintAuthority, rTokenMint, rTokenUserAccount.publicKey, 10 * 10 ** 9);
    const cTokenUserAccount = await generateTokenAccount(provider, cTokenMint, swapUser.publicKey);
    await mintToAccount(provider, cTokenMintAuthority, cTokenMint, cTokenUserAccount.publicKey, 1 * 10 ** 9);

    // these are too low so round down to 0 (similar to .rs test_taki)
    await assert.rejects(program.rpc.swap(
      new anchor.BN("1"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: rTokenUserAccount.publicKey,
          swapSource: rTokenSwapAccount.publicKey,
          swapDestination: cTokenSwapAccount.publicKey,
          destination: cTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    ));

    await assert.rejects(program.rpc.swap(
      new anchor.BN("1000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: rTokenUserAccount.publicKey,
          swapSource: rTokenSwapAccount.publicKey,
          swapDestination: cTokenSwapAccount.publicKey,
          destination: cTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    ));

    await assert.rejects(program.rpc.swap(
      new anchor.BN("1000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: rTokenUserAccount.publicKey,
          swapSource: rTokenSwapAccount.publicKey,
          swapDestination: cTokenSwapAccount.publicKey,
          destination: cTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    ));

    await assert.rejects(program.rpc.swap(
      new anchor.BN("10000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: rTokenUserAccount.publicKey,
          swapSource: rTokenSwapAccount.publicKey,
          swapDestination: cTokenSwapAccount.publicKey,
          destination: cTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    ));

    // .133 RLY, minimum to get any out
    let swapTx = await program.rpc.swap(
      new anchor.BN("133000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: rTokenUserAccount.publicKey,
          swapSource: rTokenSwapAccount.publicKey,
          swapDestination: cTokenSwapAccount.publicKey,
          destination: cTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    );

    console.log("Your transaction signature", swapTx);
    // 133000000 RLY in, 37837837 TAKI out

    // user RLY goes from 10e9 -> 9.867e9
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount.publicKey)).amount.toString(),
      "9.867000000".replace(".", ""));
    // swap's RLY balance goes from 0 -> .133 RLY
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      ".133000000".replace(".", ""));
    // user TAKI goes from 1 -> 1.0378
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount.publicKey)).amount.toString(),
      "1.037837837".replace(".", ""));
    // swap's TAKI balance goes from 5 -> 4.962
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "4.962162163".replace(".", ""));

    // give user 5 more RLY and give swap 5 more TAKI for the next swap
    await mintToAccount(provider, rTokenMintAuthority, rTokenMint, rTokenUserAccount.publicKey, 5 * 10 ** 9);
    await mintToAccount(provider, cTokenMintAuthority, cTokenMint, cTokenSwapAccount.publicKey, 5 * 10 ** 9);

    // 1 RLY
    swapTx = await program.rpc.swap(
      new anchor.BN("1000000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: rTokenUserAccount.publicKey,
          swapSource: rTokenSwapAccount.publicKey,
          swapDestination: cTokenSwapAccount.publicKey,
          destination: cTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    );
    console.log("Your transaction signature", swapTx);

    // 1000000000 RLY in 227027027 TAKI out

    // user RLY goes from 14.867e9 -> 13.867e9
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount.publicKey)).amount.toString(),
      "13.867000000".replace(".", ""));
    // swap's RLY balance goes from .133 -> 1.133 RLY
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      "1.133000000".replace(".", ""));
    // user TAKI goes from 1.037 -> 1.264
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount.publicKey)).amount.toString(),
      "1.264864864".replace(".", ""));
    // swap's TAKI balance goes from 9.962 -> 9.735
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "9.735135136".replace(".", ""));

    // give user 5K more RLY and give swap 5K more TAKI for the next swap
    await mintToAccount(provider, rTokenMintAuthority, rTokenMint, rTokenUserAccount.publicKey, 5000 * 10 ** 9);
    await mintToAccount(provider, cTokenMintAuthority, cTokenMint, cTokenSwapAccount.publicKey, 5000 * 10 ** 9);

    // 1K RLY
    swapTx = await program.rpc.swap(
      new anchor.BN("1000000000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: rTokenUserAccount.publicKey,
          swapSource: rTokenSwapAccount.publicKey,
          swapDestination: cTokenSwapAccount.publicKey,
          destination: cTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    );
    console.log("Your transaction signature", swapTx);

    // 1000.000000000 RLY in 285.675675675 TAKI out

    // user RLY goes from 5013.867 -> 4013.867
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount.publicKey)).amount.toString(),
      "4013.867000000".replace(".", ""));
    // swap's RLY balance goes from 1.133 RLY -> 1001.133
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      "1001.133000000".replace(".", ""));
    // user TAKI goes from 1.264 -> 286.94
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount.publicKey)).amount.toString(),
      "286.940540539".replace(".", ""));
    // swap's TAKI balance goes from 5009.735 -> 4724.059
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "4724.059459461".replace(".", ""));

    // give user 5MM more RLY and give swap 5MM more TAKI for the next swap
    await mintToAccount(provider, rTokenMintAuthority, rTokenMint, rTokenUserAccount.publicKey, 5000000 * 10 ** 9);
    await mintToAccount(provider, cTokenMintAuthority, cTokenMint, cTokenSwapAccount.publicKey, 5000000 * 10 ** 9);

    // 1MM RLY
    swapTx = await program.rpc.swap(
      new anchor.BN("1000000000000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: rTokenUserAccount.publicKey,
          swapSource: rTokenSwapAccount.publicKey,
          swapDestination: cTokenSwapAccount.publicKey,
          destination: cTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    );
    console.log("Your transaction signature", swapTx);

    // 1000000.000000000 RLY in 285406.081081081 TAKI out

    // user RLY goes from 5004013.867 -> 4004013.867
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount.publicKey)).amount.toString(),
      "4004013.867000000".replace(".", ""));
    // swap's RLY balance goes from 1001.133 -> 1001001.133
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      "1001001.133000000".replace(".", ""));
    // user TAKI goes from 286.94 -> 285693.021
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount.publicKey)).amount.toString(),
      "285693.021621620".replace(".", ""));
    // swap's TAKI balance goes from 5004724.059 -> 4719317.978
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "4719317.978378380".replace(".", ""));

    // give user 100M more RLY and give swap 30M more TAKI for the next swap (need to mint this in a loop since
    // we can only mint up to 2^53 tokens at a time - would be nice to test 1B but this already takes a while to mint)
    for (let i = 0; i < 20; i++) {
      await mintToAccount(provider, rTokenMintAuthority, rTokenMint, rTokenUserAccount.publicKey, 5000000 * 10 ** 9);
    }
    for (let i = 0; i < 6; i++) {
      await mintToAccount(provider, cTokenMintAuthority, cTokenMint, cTokenSwapAccount.publicKey, 5000000 * 10 ** 9);
    }

    // 100M RLY
    swapTx = await program.rpc.swap(
      new anchor.BN("100000000000000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: rTokenUserAccount.publicKey,
          swapSource: rTokenSwapAccount.publicKey,
          swapDestination: cTokenSwapAccount.publicKey,
          destination: cTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    );
    console.log("Your transaction signature", swapTx);

    // 1000000000.000000000 RLY in 25969203.664864864 TAKI out

    // user RLY goes from 104004013.867 -> 4004013.867
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount.publicKey)).amount.toString(),
      "4004013.867000000".replace(".", ""));
    // swap's RLY balance goes from 1001001.133 -> 101001001.133
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      "101001001.133000000".replace(".", ""));
    // user TAKI goes from 285693.021 -> 26254896.686
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount.publicKey)).amount.toString(),
      "26254896.686486484".replace(".", ""));
    // swap's TAKI balance goes from 34719317.978378380 -> 8750114.313513516
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "8750114.313513516".replace(".", ""));

    // also do b->a tests here too (make sure no compute issues)

    // swap back 26MM TAKI
    swapTx = await program.rpc.swap(
      new anchor.BN("26000000000000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: cTokenUserAccount.publicKey,
          swapSource: cTokenSwapAccount.publicKey,
          swapDestination: rTokenSwapAccount.publicKey,
          destination: rTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    )

    console.log("Your transaction signature", swapTx);

    // 26000000.000000000 TAKI in 100108007.010786866 RLY out

    // user RLY goes from 4004013.867 -> 104112020.877786866
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount.publicKey)).amount.toString(),
      "104112020.877786866".replace(".", ""));
    // swap's RLY balance goes from 101001001.133 -> 892994.122213134
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      "892994.122213134".replace(".", ""));
    // user TAKI goes from 26254896.686 -> 254896.686
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount.publicKey)).amount.toString(),
      "254896.686486484".replace(".", ""));
    // swap's TAKI balance goes from 8750114.313513516 -> 34750114.313513516
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "34750114.313513516".replace(".", ""));

    // swap back 254K TAKI
    swapTx = await program.rpc.swap(
      new anchor.BN("254000000000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: cTokenUserAccount.publicKey,
          swapSource: cTokenSwapAccount.publicKey,
          swapDestination: rTokenSwapAccount.publicKey,
          destination: rTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    )

    console.log("Your transaction signature", swapTx);

    // 254000.000000000 TAKI in 889858.527823522 RLY out

    // user RLY goes from 104112020.877786866 -> 105001879.40561038
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount.publicKey)).amount.toString(),
      "105001879.405610388".replace(".", ""));
    // swap's RLY balance goes from 892994.122213134 -> 3135.594389612
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      "3135.594389612".replace(".", ""));
    // user TAKI goes from 254896.686 -> 896.686
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount.publicKey)).amount.toString(),
      "896.686486484".replace(".", ""));
    // swap's TAKI balance goes from 34750114.313513516 -> 35004114.313513516
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "35004114.313513516".replace(".", ""));

    // swap back 890 TAKI
    swapTx = await program.rpc.swap(
      new anchor.BN("890000000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: cTokenUserAccount.publicKey,
          swapSource: cTokenSwapAccount.publicKey,
          swapDestination: rTokenSwapAccount.publicKey,
          destination: rTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    )

    console.log("Your transaction signature", swapTx);

    // 890.000000000 TAKI in 3114.991686449 RLY out

    // user RLY goes from 105001879.405610388 -> 105004994.397296837
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount.publicKey)).amount.toString(),
      "105004994.397296837".replace(".", ""));
    // swap's RLY balance goes from 3135.594389612 -> 20.602703163
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      "20.602703163".replace(".", ""));
    // user TAKI goes from 896.686 -> 6.686
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount.publicKey)).amount.toString(),
      "6.686486484".replace(".", ""));
    // swap's TAKI balance goes from 35004114.313513516 -> 35005004.313513516
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "35005004.313513516".replace(".", ""));

    // swap back 5 TAKI
    swapTx = await program.rpc.swap(
      new anchor.BN("5000000000"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: cTokenUserAccount.publicKey,
          swapSource: cTokenSwapAccount.publicKey,
          swapDestination: rTokenSwapAccount.publicKey,
          destination: rTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    )

    console.log("Your transaction signature", swapTx);

    // 5.000000000 TAKI in 17.443243691 RLY out

    // user RLY goes from 105004994.397296837 -> 105005011.840540528
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount.publicKey)).amount.toString(),
      "105005011.840540528".replace(".", ""));
    // swap's RLY balance goes from 20.602703163 -> 3.159459472
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      "3.159459472".replace(".", ""));
    // user TAKI goes from 6.686 -> 1.686
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount.publicKey)).amount.toString(),
      "1.686486484".replace(".", ""));
    // swap's TAKI balance goes from 35005004.313513516 -> 35005009.313513516
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "35005009.313513516".replace(".", ""));

    // put rest of the TAKI in
    swapTx = await program.rpc.swap(
      new anchor.BN("1686486484"),
      new anchor.BN(0),
      {
        accounts: {
          tokenSwap: tokenSwap.publicKey,
          swapAuthority: swapAuthority,
          userTransferAuthority: swapUser.publicKey,
          source: cTokenUserAccount.publicKey,
          swapSource: cTokenSwapAccount.publicKey,
          swapDestination: rTokenSwapAccount.publicKey,
          destination: rTokenUserAccount.publicKey,
          poolMint: poolTokenMint.publicKey,
          poolFee: feeTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [swapUser]
      },
    )

    console.log("Your transaction signature", swapTx);

    // due to rounding, only 0.908108108 TAKI taken in 3.159459472 RLY out

    // user RLY goes from 105005011.840540528 -> 105005015
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenUserAccount.publicKey)).amount.toString(),
      "105005015.000000000".replace(".", ""));
    // swap's RLY balance goes from 20.602703163 -> 3.159459472
    assert.strictEqual(
      (await rToken.getAccountInfo(rTokenSwapAccount.publicKey)).amount.toString(),
      "0".replace(".", ""));
    // user TAKI goes from 1.686 -> 0.778378376
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenUserAccount.publicKey)).amount.toString(),
      ".778378376".replace(".", ""));
    // swap's TAKI balance goes from 35005009.313513516 -> 35005010.221621624
    assert.strictEqual(
      (await cToken.getAccountInfo(cTokenSwapAccount.publicKey)).amount.toString(),
      "35005010.221621624".replace(".", ""));

    // note user is left with a bit less than the 10 RLY and 1 TAKI they started with due to sqrt rounding
  });
});
