import * as anchor from '@project-serum/anchor';
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

  it('Is initialized!', async () => {

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
    const aTokenAccount = await generateTokenAccount(provider, aTokenMint, swapAuthority);
    await mintToAccount(provider, aTokenMintAuthority, aTokenMint, aTokenAccount.publicKey, 1000);
    const bTokenAccount = await generateTokenAccount(provider, bTokenMint, swapAuthority);
    await mintToAccount(provider, bTokenMintAuthority, bTokenMint, bTokenAccount.publicKey, 2000);

    ///   4. `[writable]` Pool Token Mint. Must be empty, owned by swap authority.
    const poolMint = await generateTokenMint(provider, swapAuthority);

    ///   5. `[]` Pool Token Account to deposit trading and withdraw fees.
    ///   Must be empty, not owned by swap authority
    const feeAuthority = await generateNewSignerAccount(provider);
    const feeTokenAccount = await generateTokenAccount(provider, poolMint, feeAuthority.publicKey);

    ///   6. `[writable]` Pool Token Account to deposit the initial pool token
    ///   supply.  Must be empty, not owned by swap authority.
    const destinationAuthority = await generateNewSignerAccount(provider);
    const destinationTokenAccount = await generateTokenAccount(provider, poolMint, destinationAuthority.publicKey);

    let trade_fee_numerator = new anchor.BN(1);
    let trade_fee_denominator = new anchor.BN(4);
    let owner_trade_fee_numerator = new anchor.BN(2);
    let owner_trade_fee_denominator = new anchor.BN(5);
    let owner_withdraw_fee_numerator = new anchor.BN(4);
    let owner_withdraw_fee_denominator = new anchor.BN(10);
    let host_fee_numerator = new anchor.BN(7);
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

    console.log(`tokenSwap.publicKey ${tokenSwap.publicKey}`);
    console.log(`swapAuthority ${swapAuthority}`);
    console.log(`aTokenMint.publicKey ${aTokenMint.publicKey}`);
    console.log(`bTokenMint.publicKey ${bTokenMint.publicKey}`);
    console.log(`poolMint.publicKey ${poolMint.publicKey}`);
    console.log(`feeTokenAccount.publicKey ${feeTokenAccount.publicKey}`);
    console.log(`destinationTokenAccount.publicKey ${destinationTokenAccount.publicKey}`);
    console.log(`TOKEN_PROGRAM_PUBKEY ${TOKEN_PROGRAM_PUBKEY}`);
    console.log(`fees ${JSON.stringify(fees)}`);

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
          tokenA: aTokenAccount.publicKey,
          tokenB: bTokenAccount.publicKey,
          pool: poolMint.publicKey,
          fee: feeTokenAccount.publicKey,
          destination: destinationTokenAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_PUBKEY,
        },
        signers: [tokenSwap],
      });
    console.log("Your transaction signature", tx);
  });
});
