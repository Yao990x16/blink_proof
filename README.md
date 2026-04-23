# BlinkProof

**Don't trust -- Blink.**

BlinkProof is a decentralized content provenance protocol built for the Solana Frontier Hackathon 2026. It targets the core authenticity problem created by generative AI: images and short-form media can now be produced, edited, and redistributed faster than viewers can verify where they came from.

The protocol combines two Solana-native primitives:

- **State Compression** to anchor low-cost perceptual-hash attestations at scale.
- **Blinks** to turn verification into a social-native action instead of a multi-step wallet workflow.

The goal is simple: if a creator, publisher, or platform wants to prove that a piece of media is authentic, they should be able to publish that proof on Solana and let any viewer verify it with one blink.

## Problem

AI-generated and AI-edited media breaks traditional trust signals.

- A reposted image no longer carries reliable origin metadata.
- Watermarks are easy to crop, remove, or fake.
- Centralized provenance databases create platform lock-in and single points of failure.
- Verification flows are too slow for social media, where trust decisions happen in seconds.

BlinkProof is designed for the moment a viewer sees content in a feed and asks: *is this real, who attested to it, and when?*

## Protocol Vision

BlinkProof does not try to store media on chain. It stores **compact attestations** that can be independently recomputed and verified.

At a high level:

1. A creator or verifier derives a perceptual hash (pHash) from a media asset.
2. The protocol packages that fingerprint with provenance metadata.
3. The attestation is committed into a compressed data structure on Solana.
4. A Blink exposes a zero-friction verification action from social surfaces.
5. Viewers inspect the attestation, issuer, timestamp, and proof path before trusting the content.

This design keeps costs low while preserving public verifiability.

## Why Solana

BlinkProof is intentionally Solana-native.

- **State Compression** gives us a practical path to high-volume attestation writes without treating every proof as a full-sized account.
- **Blinks** reduce the UX gap between “I saw this on social” and “I verified this on chain.”
- **Fast finality and low fees** fit the cadence of social verification rather than slow settlement-style interaction.
- **Composable programs** let the protocol expand later into reputation, delegated attestations, media registries, and downstream indexing.

## Core Architecture

### 1. Media Fingerprinting

BlinkProof will use perceptual hashes instead of raw file bytes as the primary media fingerprint primitive.

- More robust than plain content hashes for resized or lightly edited images.
- Better aligned with authenticity workflows for social content.
- Small enough to fit efficiently into compressed attestation flows.

The pHash is expected to be computed off chain. The on-chain program verifies structure, authority, and inclusion logic, not expensive image processing.

### 2. Compressed Attestations

Each proof is intended to be represented as a compressed leaf rather than a standalone account.

Planned leaf contents include:

- Media pHash
- Issuer / attestor public key
- Subject or creator reference
- Timestamp / slot context
- Optional content URI or external metadata pointer
- Optional schema / version field

Phase 1 is expected to build this on top of `spl-account-compression`.

### 3. Verification via Blinks

A Blink should let a user verify content directly from a post, message, or embed.

Expected Blink flow:

1. User opens a Blink from a social surface.
2. The Blink resolves the candidate media fingerprint and attestation reference.
3. The client fetches the compressed proof context.
4. The user sees a clear result:
   - verified
   - unverified
   - conflicting attestations
   - missing provenance

The protocol thesis is that provenance only matters if verification is frictionless.

## Project Principle

BlinkProof is built around a simple product belief:

> Authenticity should be verifiable at the speed of a social click.

Or more directly:

> **Don't trust -- Blink.**
