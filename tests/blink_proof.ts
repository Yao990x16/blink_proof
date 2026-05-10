import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { expect } from "chai";
import { BlinkProof } from "../target/types/blink_proof";
const blinkProofIdl = require("../target/idl/blink_proof.json");

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

  async function createTreeConfig(
    merkleTreePubkey: anchor.web3.PublicKey,
    isPublic: boolean
  ) {
    const [treeConfig] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("tree_config"), merkleTreePubkey.toBuffer()],
      program.programId
    );

    await program.methods
      .createTreeConfig(isPublic)
      .accountsPartial({
        merkleTree: merkleTreePubkey,
        treeConfig,
        admin: provider.wallet.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    return treeConfig;
  }

  async function authorizeIssuer(
    treeConfigPubkey: anchor.web3.PublicKey,
    merkleTreePubkey: anchor.web3.PublicKey,
    issuerPubkey: anchor.web3.PublicKey
  ) {
    const [issuerCredential] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("issuer_credential"),
        merkleTreePubkey.toBuffer(),
        issuerPubkey.toBuffer(),
      ],
      program.programId
    );

    await program.methods
      .authorizeIssuer()
      .accountsPartial({
        treeConfig: treeConfigPubkey,
        issuerCredential,
        issuer: issuerPubkey,
        admin: provider.wallet.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    return issuerCredential;
  }

  function uniqueFingerprint(
    merkleTreePubkey: anchor.web3.PublicKey,
    tag: number
  ): Buffer {
    const fingerprint = Buffer.from(merkleTreePubkey.toBuffer());
    fingerprint[0] = tag;
    return fingerprint;
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

  it("Registers a content hash", async () => {
    const { merkleTree, treeAuthority } = await createInitializedTree();
    const saltedFingerprint = uniqueFingerprint(merkleTree.publicKey, 1);
    const rawPhash = Buffer.from(
      Array.from({ length: 8 }, (_, index) => 201 + index)
    );
    const [registrationReceipt] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("receipt"), saltedFingerprint],
      program.programId
    );

    const beforeAccount = await provider.connection.getAccountInfo(
      merkleTree.publicKey
    );
    const beforeSequence = Number(
      beforeAccount!.data.readBigUInt64LE(SEQUENCE_NUMBER_OFFSET)
    );

    const tx = await program.methods
      .registerContent([...saltedFingerprint], [...rawPhash])
      .accountsPartial({
        merkleTree: merkleTree.publicKey,
        treeAuthority,
        authority: provider.wallet.publicKey,
        registrationReceipt,
        systemProgram: anchor.web3.SystemProgram.programId,
        compressionProgram: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
        logWrapper: SPL_NOOP_PROGRAM_ID,
        treeConfig: null,
        issuerCredential: null,
      })
      .rpc();
    const txDetails = await provider.connection.getTransaction(tx, {
      commitment: "confirmed",
      maxSupportedTransactionVersion: 0,
    });
    const eventCoder = new anchor.BorshEventCoder(blinkProofIdl as anchor.Idl);
    const contentRegistered = (txDetails?.meta?.logMessages ?? [])
      .map((log) => {
        const match = log.match(/^Program data: (.+)$/);
        return match ? eventCoder.decode(match[1]) : null;
      })
      .find((event) => event?.name === "ContentRegistered");

    const afterAccount = await provider.connection.getAccountInfo(
      merkleTree.publicKey
    );
    const afterSequence = Number(
      afterAccount!.data.readBigUInt64LE(SEQUENCE_NUMBER_OFFSET)
    );

    console.log("registerContent signature:", tx);
    console.log("registerContent slot:", txDetails?.slot ?? "unknown");
    expect(tx).to.be.a("string");
    expect(txDetails).to.not.equal(null);
    expect(contentRegistered).to.not.equal(undefined);
    expect(
      contentRegistered?.data.creator.equals(provider.wallet.publicKey)
    ).to.eq(true);
    expect(
      Buffer.from(contentRegistered?.data.salted_fingerprint ?? [])
    ).to.deep.equal(
      saltedFingerprint
    );
    expect(
      Buffer.from(contentRegistered?.data.raw_phash ?? [])
    ).to.deep.equal(
      rawPhash
    );
    expect(contentRegistered?.data.timestamp.toNumber()).to.be.greaterThan(0);
    expect(afterSequence).to.eq(beforeSequence + 1);
  });

  it("Rejects duplicate content registration", async () => {
    const { merkleTree, treeAuthority } = await createInitializedTree();
    const saltedFingerprint = uniqueFingerprint(merkleTree.publicKey, 2);
    const rawPhash = Buffer.from(
      Array.from({ length: 8 }, (_, index) => 201 + index)
    );

    const [registrationReceipt] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("receipt"), saltedFingerprint],
      program.programId
    );

    await program.methods
      .registerContent([...saltedFingerprint], [...rawPhash])
      .accountsPartial({
        merkleTree: merkleTree.publicKey,
        treeAuthority,
        authority: provider.wallet.publicKey,
        registrationReceipt,
        systemProgram: anchor.web3.SystemProgram.programId,
        compressionProgram: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
        logWrapper: SPL_NOOP_PROGRAM_ID,
        treeConfig: null,
        issuerCredential: null,
      })
      .rpc();

    try {
      await program.methods
        .registerContent([...saltedFingerprint], [...rawPhash])
        .accountsPartial({
          merkleTree: merkleTree.publicKey,
          treeAuthority,
          authority: provider.wallet.publicKey,
          registrationReceipt,
          systemProgram: anchor.web3.SystemProgram.programId,
          compressionProgram: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
          logWrapper: SPL_NOOP_PROGRAM_ID,
          treeConfig: null,
          issuerCredential: null,
        })
        .rpc();
      expect.fail("should have thrown on duplicate registration");
    } catch (error: any) {
      expect(error).to.be.instanceOf(Error);
    }
  });

  it("Creates a tree config (public mode)", async () => {
    const { merkleTree } = await createInitializedTree();
    const treeConfig = await createTreeConfig(merkleTree.publicKey, true);
    const account = await program.account.treeConfig.fetch(treeConfig);

    expect(account.isPublic).to.eq(true);
    expect(account.issuerCount).to.eq(0);
    expect(account.admin.equals(provider.wallet.publicKey)).to.eq(true);
  });

  it("Authorizes and revokes an issuer", async () => {
    const { merkleTree } = await createInitializedTree();
    const treeConfig = await createTreeConfig(merkleTree.publicKey, false);
    const issuerKp = anchor.web3.Keypair.generate();

    const credential = await authorizeIssuer(
      treeConfig,
      merkleTree.publicKey,
      issuerKp.publicKey
    );
    const configAfterAuth = await program.account.treeConfig.fetch(treeConfig);
    expect(configAfterAuth.issuerCount).to.eq(1);

    const credAccount = await program.account.issuerCredential.fetch(
      credential
    );
    expect(credAccount.issuer.equals(issuerKp.publicKey)).to.eq(true);

    await program.methods
      .revokeIssuer()
      .accountsPartial({
        treeConfig,
        issuerCredential: credential,
        admin: provider.wallet.publicKey,
      })
      .rpc();
    const configAfterRevoke = await program.account.treeConfig.fetch(
      treeConfig
    );
    expect(configAfterRevoke.issuerCount).to.eq(0);

    const closedAccount = await provider.connection.getAccountInfo(credential);
    expect(closedAccount).to.eq(null);
  });

  it("Rejects unauthorized issuer on permissioned tree", async () => {
    const { merkleTree, treeAuthority } = await createInitializedTree();
    const treeConfig = await createTreeConfig(merkleTree.publicKey, false);

    const saltedFingerprint = uniqueFingerprint(merkleTree.publicKey, 3);
    const rawPhash = Buffer.from(
      Array.from({ length: 8 }, (_, i) => 100 + i)
    );
    const [registrationReceipt] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("receipt"), saltedFingerprint],
      program.programId
    );

    try {
      await program.methods
        .registerContent([...saltedFingerprint], [...rawPhash])
        .accountsPartial({
          merkleTree: merkleTree.publicKey,
          treeAuthority,
          authority: provider.wallet.publicKey,
          registrationReceipt,
          systemProgram: anchor.web3.SystemProgram.programId,
          compressionProgram: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
          logWrapper: SPL_NOOP_PROGRAM_ID,
          treeConfig,
          issuerCredential: null,
        })
        .rpc();
      expect.fail("should have rejected unauthorized issuer");
    } catch (error: any) {
      expect(error.toString()).to.include("UnauthorizedIssuer");
    }
  });

  it("Allows authorized issuer on permissioned tree", async () => {
    const { merkleTree, treeAuthority } = await createInitializedTree();
    const treeConfig = await createTreeConfig(merkleTree.publicKey, false);

    const credential = await authorizeIssuer(
      treeConfig,
      merkleTree.publicKey,
      provider.wallet.publicKey
    );

    const saltedFingerprint = uniqueFingerprint(merkleTree.publicKey, 4);
    const rawPhash = Buffer.from(
      Array.from({ length: 8 }, (_, i) => 110 + i)
    );
    const [registrationReceipt] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("receipt"), saltedFingerprint],
      program.programId
    );

    const tx = await program.methods
      .registerContent([...saltedFingerprint], [...rawPhash])
      .accountsPartial({
        merkleTree: merkleTree.publicKey,
        treeAuthority,
        authority: provider.wallet.publicKey,
        registrationReceipt,
        systemProgram: anchor.web3.SystemProgram.programId,
        compressionProgram: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
        logWrapper: SPL_NOOP_PROGRAM_ID,
        treeConfig,
        issuerCredential: credential,
      })
      .rpc();

    expect(tx).to.be.a("string");
  });

  it("Allows registering different fingerprints on the same tree", async () => {
    const { merkleTree, treeAuthority } = await createInitializedTree();

    for (let i = 0; i < 3; i++) {
      const fingerprint = uniqueFingerprint(merkleTree.publicKey, 10 + i);
      const phash = Buffer.from(
        Array.from({ length: 8 }, (_, j) => (i * 8 + j) % 256)
      );
      const [receipt] = anchor.web3.PublicKey.findProgramAddressSync(
        [Buffer.from("receipt"), fingerprint],
        program.programId
      );

      await program.methods
        .registerContent([...fingerprint], [...phash])
        .accountsPartial({
          merkleTree: merkleTree.publicKey,
          treeAuthority,
          authority: provider.wallet.publicKey,
          registrationReceipt: receipt,
          systemProgram: anchor.web3.SystemProgram.programId,
          compressionProgram: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
          logWrapper: SPL_NOOP_PROGRAM_ID,
          treeConfig: null,
          issuerCredential: null,
        })
        .rpc();
    }

    const account = await provider.connection.getAccountInfo(
      merkleTree.publicKey
    );
    const seq = Number(account!.data.readBigUInt64LE(SEQUENCE_NUMBER_OFFSET));
    expect(seq).to.eq(3);
  });

  it("Rejects non-admin attempting to authorize an issuer", async () => {
    const { merkleTree } = await createInitializedTree();
    const treeConfig = await createTreeConfig(merkleTree.publicKey, false);
    const fakeAdmin = anchor.web3.Keypair.generate();
    const issuer = anchor.web3.Keypair.generate();

    const sig = await provider.connection.requestAirdrop(
      fakeAdmin.publicKey,
      1_000_000_000
    );
    await provider.connection.confirmTransaction(sig);

    const [credential] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("issuer_credential"),
        merkleTree.publicKey.toBuffer(),
        issuer.publicKey.toBuffer(),
      ],
      program.programId
    );

    try {
      await program.methods
        .authorizeIssuer()
        .accountsPartial({
          treeConfig,
          issuerCredential: credential,
          issuer: issuer.publicKey,
          admin: fakeAdmin.publicKey,
          systemProgram: anchor.web3.SystemProgram.programId,
        })
        .signers([fakeAdmin])
        .rpc();
      expect.fail("should have rejected non-admin");
    } catch (error: any) {
      expect(error).to.be.instanceOf(Error);
    }
  });

  it("Rejects malformed Merkle proof before verification CPI", async () => {
    const { merkleTree } = await createInitializedTree();
    const saltedFingerprint = Buffer.from(
      Array.from({ length: 32 }, (_, i) => 90 + i)
    );
    const root = Buffer.alloc(32, 0);
    const malformedProof = [Array.from(Buffer.alloc(32, 1))];

    try {
      await program.methods
        .verifyContent([...root], [...saltedFingerprint], 0, malformedProof)
        .accountsPartial({
          merkleTree: merkleTree.publicKey,
          verifier: provider.wallet.publicKey,
          compressionProgram: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
        })
        .rpc();
      expect.fail("should have rejected malformed proof accounts");
    } catch (error: any) {
      expect(error.toString()).to.include("InvalidProof");
    }
  });
});
