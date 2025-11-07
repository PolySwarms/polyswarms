import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import { assert } from "chai";

describe("polyswarms", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Polyswarms as Program;

  const marketId = new anchor.BN(1);
  const creator = (provider.wallet as any).payer as any;
  const resolver = Keypair.generate();
  let marketPda: PublicKey;
  let yesPool: PublicKey;
  let noPool: PublicKey;

  it("create market", async () => {
    [marketPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("market"), provider.wallet.publicKey.toBuffer(), Buffer.from(new Uint8Array(new anchor.BN(1).toArray("le", 8)))],
      program.programId
    );
    [yesPool] = PublicKey.findProgramAddressSync([Buffer.from("pool"), marketPda.toBuffer(), Buffer.from("YES")], program.programId);
    [noPool]  = PublicKey.findProgramAddressSync([Buffer.from("pool"), marketPda.toBuffer(), Buffer.from("NO")], program.programId);

    const feeVault = PublicKey.findProgramAddressSync([Buffer.from("fee_vault")], program.programId)[0];

    await program.methods.createMarket({
      marketId: marketId,
      resolver: resolver.publicKey,
      feeBps: 100,          // 1%
      closeTs: new anchor.BN(Math.floor(Date.now()/1000) + 3600),
    }).accounts({
      creator: provider.wallet.publicKey,
      resolver: resolver.publicKey,
      market: marketPda,
      yesPool,
      noPool,
      feeVault,
      systemProgram: SystemProgram.programId
    }).rpc();

    const m: any = await program.account.market.fetch(marketPda);
    assert.equal(m.status.open, true);
  });
});
