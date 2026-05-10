use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use img_hash::{HashAlg, HasherConfig};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::task;

const PHASH_HASH_SIZE: u32 = 8;
const SALT: &[u8] = b"BLINK_PROOF_V1_2026";

pub async fn calculate_phash(image_url: String) -> Result<([u8; 32], [u8; 8])> {
    let image_bytes = if let Some(data) = parse_data_url(&image_url) {
        data
    } else {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .context("failed to build HTTP client")?;
        let response = client
            .get(&image_url)
            .send()
            .await
            .with_context(|| format!("failed to download image from {image_url}"))?;

        let response = response
            .error_for_status()
            .with_context(|| format!("image request returned an error status for {image_url}"))?;

        response
            .bytes()
            .await
            .with_context(|| format!("failed to read image bytes from {image_url}"))?
            .to_vec()
    };

    task::spawn_blocking(move || decode_and_hash_image(image_bytes))
        .await
        .context("image hashing task panicked or was cancelled")?
}

fn parse_data_url(url: &str) -> Option<Vec<u8>> {
    let data_part = url.strip_prefix("data:")?;
    let (_, encoded) = data_part.split_once(";base64,")?;
    STANDARD.decode(encoded.trim()).ok()
}

fn decode_and_hash_image(image_bytes: Vec<u8>) -> Result<([u8; 32], [u8; 8])> {
    let _decoded_image = image::load_from_memory(&image_bytes)
        .context("failed to decode image bytes with the image crate")?;
    let hash_image = img_hash::image::load_from_memory(&image_bytes)
        .context("failed to decode image bytes into img_hash compatible image data")?;

    // img_hash 3.2 does not expose a DCT-specific enum variant, so we use its
    // strongest built-in perceptual mode to keep the hashing pipeline stable here.
    let hasher = HasherConfig::new()
        .hash_alg(HashAlg::DoubleGradient)
        .hash_size(PHASH_HASH_SIZE, PHASH_HASH_SIZE)
        .to_hasher();
    let image_hash = hasher.hash_image(&hash_image);
    let hash_bytes = image_hash.as_bytes();
    let raw_phash = extract_raw_phash_bytes(hash_bytes)?;

    Ok((salted_sha256_fingerprint(&raw_phash), raw_phash))
}

fn extract_raw_phash_bytes(hash_bytes: &[u8]) -> Result<[u8; 8]> {
    let raw_phash: [u8; 8] = hash_bytes
        .get(..8)
        .context("img_hash returned fewer than 8 bytes for the perceptual hash")?
        .try_into()
        .context("failed to convert perceptual hash bytes into a fixed 8-byte array")?;

    Ok(raw_phash)
}

/// Maps the compact 64-bit perceptual hash into a stable 32-byte fingerprint.
///
/// The salt prevents this layer from being a transparent raw pHash export, which
/// gives BlinkProof room to version the fingerprinting pipeline and reduces
/// accidental cross-system equivalence with external unsalted pHash datasets.
fn salted_sha256_fingerprint(raw_phash: &[u8; 8]) -> [u8; 32] {
    let digest = Sha256::digest([SALT, raw_phash].concat());
    let mut fingerprint = [0u8; 32];
    fingerprint.copy_from_slice(&digest);

    fingerprint
}
