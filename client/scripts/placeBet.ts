import "dotenv/config";
import * as anchor from "@coral-xyz/anchor";
import { Keypair, SystemProgram, PublicKey } from "@solana/web3.js";
import fs from "fs";

const [market, side, amount] = process.argv.slice(2);
if (!market || !side || !amount) {
  console.log("Usage: yarn bet <MARKET_PUBKEY> <YES|NO> <SOL>");
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
const [pool] = PublicKey.findProgramAddressSync([Buffer.from("pool"), marketPk.toBuffer(), Buffer.from(side)], PROGRAM_ID);
const [bet]  = PublicKey.findProgramAddressSync([Buffer.from("bet"), marketPk.toBuffer(), kp.publicKey.toBuffer(), Buffer.from(side)], PROGRAM_ID);

await program.methods.placeBet(side === "YES" ? { yes: {} } as any : { no: {} } as any, new anchor.BN(Number(amount) * 1e9))
  .accounts({
    user: kp.publicKey,
    market: marketPk,
    pool,
    bet,
    systemProgram: SystemProgram.programId
  }).rpc();

console.log(`Placed ${amount} SOL on ${side}`);
