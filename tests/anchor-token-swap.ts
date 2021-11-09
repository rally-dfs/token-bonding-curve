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
      8,
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

describe('anchor-token-swap', () => {

  // Configure the client to use the local cluster.
  const provider = anchor.Provider.env();
  anchor.setProvider(provider);

  it('should perform constant price swap!', async () => {

    const program = anchor.workspace.AnchorTokenSwap;

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
      "0.00000000".replace(".", ""));
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
          poolTokenMint: poolTokenMint.publicKey,
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
});
