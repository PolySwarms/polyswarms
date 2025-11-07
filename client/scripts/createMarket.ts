import "dotenv/config";
import * as anchor from "@coral-xyz/anchor";
import { Keypair, SystemProgram, PublicKey } from "@solana/web3.js";
import fs from "fs";

const RPC = process.env.RPC_URL!;
const PROGRAM_ID = new PublicKey(process.env.PROGRAM_ID!);
const WALLET_PATH = (process.env.WALLET || `${process.env.HOME}/.config/solana/id.json`) as string;

const kp = Keypair.fromSecretKey(
  new Uint8Array(JSON.parse(fs.readFileSync(WALLET_PATH, "utf8")))
);

const provider = new anchor.AnchorProvider(
  new anchor.web3.Connection(RPC, "confirmed"),
  new anchor.Wallet(kp),
  {}
);
anchor.setProvider(provider);

const idl = await anchor.Program.fetchIdl(PROGRAM_ID, provider);
const program = new anchor.Program(idl!, PROGRAM_ID, provider);

const marketId = new anchor.BN(Date.now()); // unique

const [marketPda] = PublicKey.findProgramAddressSync(
  [Buffer.from("market"), kp.publicKey.toBuffer(), Buffer.from(new Uint8Array(marketId.toArray("le", 8)))],
  PROGRAM_ID
);
const [yesPool] = PublicKey.findProgramAddressSync([Buffer.from("pool"), marketPda.toBuffer(), Buffer.from("YES")], PROGRAM_ID);
const [noPool]  = PublicKey.findProgramAddressSync([Buffer.from("pool"), marketPda.toBuffer(), Buffer.from("NO")], PROGRAM_ID);
const [feeVault] = PublicKey.findProgramAddressSync([Buffer.from("fee_vault")], PROGRAM_ID);

const resolver = Keypair.generate();

await program.methods.createMarket({
  marketId,
  resolver: resolver.publicKey,
  feeBps: 100,
  closeTs: new anchor.BN(Math.floor(Date.now()/1000) + 3600),
}).accounts({
  creator: kp.publicKey,
  resolver: resolver.publicKey,
  market: marketPda,
  yesPool,
  noPool,
  feeVault,
  systemProgram: SystemProgram.programId
}).rpc();

console.log("Market:", marketPda.toBase58());
console.log("YES Pool:", yesPool.toBase58());
console.log("NO Pool :", noPool.toBase58());
console.log("Resolver :", resolver.publicKey.toBase58());
