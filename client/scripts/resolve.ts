import "dotenv/config";
import * as anchor from "@coral-xyz/anchor";
import { Keypair, SystemProgram, PublicKey } from "@solana/web3.js";
import fs from "fs";

const [market, outcome] = process.argv.slice(2);
if (!market || !outcome) {
  console.log("Usage: yarn resolve <MARKET_PUBKEY> <YES|NO>");
  process.exit(1);
}

const RPC = process.env.RPC_URL!;
const PROGRAM_ID = new PublicKey(process.env.PROGRAM_ID!);
const WALLET_PATH = (process.env.WALLET || `${process.env.HOME}/.config/solana/id.json`) as string;

const kp = Keypair.fromSecretKey(new Uint8Array(JSON.parse(fs.readFileSync(WALLET_PATH, "utf8"))));
const provider = new anchor.AnchorProvider(new anchor.web3.Connection(RPC, "confirmed"), new anchor.Wallet(kp), {});
anchor.setProvider(provider);

const idl = await anchor.Program.fetchIdl(PROGRAM_ID, provider);
const program = new anchor.Program(idl!, PROGRAM_ID, provider);

const marketPk = new PublicKey(market);
const [yesPool] = PublicKey.findProgramAddressSync([Buffer.from("pool"), marketPk.toBuffer(), Buffer.from("YES")], PROGRAM_ID);
const [noPool]  = PublicKey.findProgramAddressSync([Buffer.from("pool"), marketPk.toBuffer(), Buffer.from("NO")], PROGRAM_ID);
const [feeVault] = PublicKey.findProgramAddressSync([Buffer.from("fee_vault")], PROGRAM_ID);

await program.methods.resolve(outcome === "YES" ? { yes: {} } as any : { no: {} } as any)
  .accounts({
    resolver: kp.publicKey,
    market: marketPk,
    yesPool,
    noPool,
    feeVault,
    systemProgram: SystemProgram.programId
  }).rpc();

console.log(`Resolved market to ${outcome}`);
