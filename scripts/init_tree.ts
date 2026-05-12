import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { BlinkProof } from "../target/types/blink_proof";
import * as fs from "fs";
import * as path from "path";

const SPL_ACCOUNT_COMPRESSION_PROGRAM_ID = new anchor.web3.PublicKey(
  "cmtDvXumGCrqC1Age74AVPhSRVXJMd8PJS91L8KbNCK"
);
const SPL_NOOP_PROGRAM_ID = new anchor.web3.PublicKey(
  "noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV"
);

function concurrentMerkleTreeAccountSize(maxDepth: number, maxBufferSize: number, canopyDepth: number): number {
  const HEADER_SIZE = 56;
  const METADATA_SIZE = 24;
  const pathSize = 32 * (maxDepth + 1) + 8;
  const changeLogSize = 32 * (maxDepth + 1) + 8;
  const canopySize = canopyDepth === 0 ? 0 : ((1 << (canopyDepth + 1)) - 2) * 32;
  return HEADER_SIZE + METADATA_SIZE + maxBufferSize * changeLogSize + pathSize + canopySize;
}

async function main() {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.blinkProof as Program<BlinkProof>;
  const maxDepth = 14;
  const maxBufferSize = 64;
  const canopyDepth = 0;

  const merkleTree = anchor.web3.Keypair.generate();
  const [treeAuthority] = anchor.web3.PublicKey.findProgramAddressSync(
    [Buffer.from("tree_authority"), merkleTree.publicKey.toBuffer()],
    program.programId
  );

  const requiredSpace = concurrentMerkleTreeAccountSize(maxDepth, maxBufferSize, canopyDepth);
  const lamports = await provider.connection.getMinimumBalanceForRentExemption(requiredSpace);

  console.log("Allocating and Initializing Merkle Tree...");
  const allocateTreeIx = anchor.web3.SystemProgram.createAccount({
    fromPubkey: provider.wallet.publicKey,
    newAccountPubkey: merkleTree.publicKey,
    lamports,
    space: requiredSpace,
    programId: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
  });

  const tx = await program.methods
    .initializeTree()
    .accountsPartial({
      merkleTree: merkleTree.publicKey,
      treeAuthority,
      compressionProgram: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
      logWrapper: SPL_NOOP_PROGRAM_ID,
    })
    .preInstructions([allocateTreeIx])
    .signers([merkleTree])
    .rpc();

  console.log("Tree created successfully!");
  console.log("Merkle Tree Pubkey:", merkleTree.publicKey.toBase58());
  console.log("Tx Signature:", tx);

  // M3: Initialize the tree config as PUBLIC so anyone can register content without issuer credentials
  console.log("Setting tree as Public...");
  const [treeConfig] = anchor.web3.PublicKey.findProgramAddressSync(
    [Buffer.from("tree_config"), merkleTree.publicKey.toBuffer()],
    program.programId
  );
  await program.methods
    .createTreeConfig(true) // isPublic = true
    .accountsPartial({
      merkleTree: merkleTree.publicKey,
      treeConfig,
    })
    .rpc();

  console.log("Tree configured as Public.");

  const envPath = path.join(__dirname, "../services/blink_action/.env");
  const envContent = `BLINK_MERKLE_TREE=${merkleTree.publicKey.toBase58()}\n`;
  fs.writeFileSync(envPath, envContent);
  console.log("✅ Wrote BLINK_MERKLE_TREE to services/blink_action/.env");
}

main().catch(console.error);
