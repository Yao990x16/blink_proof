import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { expect } from "chai";
import { BlinkProof } from "../target/types/blink_proof";

const SPL_ACCOUNT_COMPRESSION_PROGRAM_ID = new anchor.web3.PublicKey(
  "cmtDvXumGCrqC1Age74AVPhSRVXJMd8PJS91L8KbNCK"
);
const SPL_NOOP_PROGRAM_ID = new anchor.web3.PublicKey(
  "noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV"
);
const CONCURRENT_MERKLE_TREE_HEADER_SIZE = 56;
const TREE_METADATA_SIZE = 24;
const ACCOUNT_TYPE_OFFSET = 0;
const HEADER_VERSION_OFFSET = 1;
const MAX_BUFFER_SIZE_OFFSET = 2;
const MAX_DEPTH_OFFSET = 6;
const AUTHORITY_OFFSET = 10;
const CREATION_SLOT_OFFSET = 42;
const SEQUENCE_NUMBER_OFFSET = CONCURRENT_MERKLE_TREE_HEADER_SIZE;

function concurrentMerkleTreePathSize(maxDepth: number): number {
  return 32 * (maxDepth + 1) + 8;
}

function concurrentMerkleTreeChangeLogSize(maxDepth: number): number {
  return 32 * (maxDepth + 1) + 8;
}

function concurrentMerkleTreeCanopySize(canopyDepth: number): number {
  if (canopyDepth === 0) {
    return 0;
  }

  return ((1 << (canopyDepth + 1)) - 2) * 32;
}

function concurrentMerkleTreeAccountSize(
  maxDepth: number,
  maxBufferSize: number,
  canopyDepth: number
): number {
  return (
    CONCURRENT_MERKLE_TREE_HEADER_SIZE +
    TREE_METADATA_SIZE +
    maxBufferSize * concurrentMerkleTreeChangeLogSize(maxDepth) +
    concurrentMerkleTreePathSize(maxDepth) +
    concurrentMerkleTreeCanopySize(canopyDepth)
  );
}

describe("blink_proof", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.blinkProof as Program<BlinkProof>;
  const maxDepth = 14;
  const maxBufferSize = 64;
  const canopyDepth = 0;

  async function createInitializedTree() {
    const merkleTree = anchor.web3.Keypair.generate();
    const [treeAuthority] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("tree_authority"), merkleTree.publicKey.toBuffer()],
      program.programId
    );

    const requiredSpace = concurrentMerkleTreeAccountSize(
      maxDepth,
      maxBufferSize,
      canopyDepth
    );
    const lamports =
      await provider.connection.getMinimumBalanceForRentExemption(
        requiredSpace
      );

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

    return { merkleTree, requiredSpace, treeAuthority, tx };
  }

  it("initializes a compressed Merkle tree with PDA authority", async () => {
    const { merkleTree, requiredSpace, treeAuthority, tx } =
      await createInitializedTree();

    const accountInfo = await provider.connection.getAccountInfo(
      merkleTree.publicKey
    );
    const accountData = accountInfo?.data;

    expect(tx).to.be.a("string");
    expect(accountInfo).to.not.equal(null);
    expect(accountInfo?.owner.equals(SPL_ACCOUNT_COMPRESSION_PROGRAM_ID)).to.eq(
      true
    );
    expect(accountInfo?.data.length).to.eq(requiredSpace);
    expect(accountData?.readUInt8(ACCOUNT_TYPE_OFFSET)).to.eq(1);
    expect(accountData?.readUInt8(HEADER_VERSION_OFFSET)).to.eq(0);
    expect(accountData?.readUInt32LE(MAX_BUFFER_SIZE_OFFSET)).to.eq(
      maxBufferSize
    );
    expect(accountData?.readUInt32LE(MAX_DEPTH_OFFSET)).to.eq(maxDepth);
    expect(
      new anchor.web3.PublicKey(
        accountData!.subarray(AUTHORITY_OFFSET, AUTHORITY_OFFSET + 32)
      ).equals(treeAuthority)
    ).to.eq(true);
    expect(Number(accountData?.readBigUInt64LE(CREATION_SLOT_OFFSET))).to.be
      .greaterThan(0);
    expect(Number(accountData?.readBigUInt64LE(SEQUENCE_NUMBER_OFFSET))).to.eq(
      0
    );
  });

  it("registers content by appending a content hash into the Merkle tree", async () => {
    const { merkleTree, treeAuthority } = await createInitializedTree();
    const contentHash = Array.from(anchor.web3.Keypair.generate().publicKey.toBytes());

    const beforeAccount = await provider.connection.getAccountInfo(
      merkleTree.publicKey
    );
    const beforeSequence = Number(
      beforeAccount!.data.readBigUInt64LE(SEQUENCE_NUMBER_OFFSET)
    );

    const tx = await program.methods
      .registerContent({ contentHash })
      .accountsPartial({
        merkleTree: merkleTree.publicKey,
        treeAuthority,
        authority: provider.wallet.publicKey,
        compressionProgram: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
        logWrapper: SPL_NOOP_PROGRAM_ID,
      })
      .rpc();

    const afterAccount = await provider.connection.getAccountInfo(
      merkleTree.publicKey
    );
    const afterSequence = Number(
      afterAccount!.data.readBigUInt64LE(SEQUENCE_NUMBER_OFFSET)
    );

    expect(tx).to.be.a("string");
    expect(afterSequence).to.eq(beforeSequence + 1);
  });
});
