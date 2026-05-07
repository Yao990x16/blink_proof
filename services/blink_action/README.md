# BlinkProof Blink Action Service

Axum backend for BlinkProof's Solana Action endpoints.

## What It Provides

- `GET /api/actions/verify`
  - Returns Solana Actions metadata for the BlinkProof verification action.
- `POST /api/actions/verify`
  - Accepts a wallet `account`
  - Derives a placeholder media pHash
  - Builds an unsigned `register_content` transaction for the `blink_proof` program
  - Returns the serialized transaction as base64
- `GET /actions.json`
  - Returns root-domain Action mapping rules required by Blink clients

## Environment

Copy `.env.example` to `.env` and set:

- `BLINK_ACTION_BIND_ADDR`
  - HTTP bind address for the Axum server
- `SOLANA_RPC_URL`
  - Solana RPC endpoint used to fetch the latest blockhash
- `BLINK_MERKLE_TREE`
  - The compressed Merkle tree account that `register_content` should append to
- `RUST_LOG`
  - Optional tracing filter

## Run

```bash
cargo run -p blink_action
```

The service listens on `http://127.0.0.1:3000` by default.

## Test Manually

```bash
curl http://127.0.0.1:3000/api/actions/verify
```

```bash
curl -X POST http://127.0.0.1:3000/api/actions/verify \
  -H 'Content-Type: application/json' \
  -d '{"account":"8k5gEuVn6si1i9xB1eY2srhpmCRm4ihHm1cpoKakuULv"}'
```

```bash
curl http://127.0.0.1:3000/actions.json
```

## Extension Points

- Replace `src/phash.rs` with a real image hash pipeline
- Extend `src/blinkproof.rs` to query existing attestations before composing a transaction
- Replace the placeholder icon URL with a production asset
