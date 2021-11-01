use std::convert::TryInto;

use lazy_static::lazy_static;
use hex_literal::hex;

use rand_core::{RngCore, CryptoRng};
use blake2::{Digest, Blake2b};

use curve25519_dalek::{
  constants::{RISTRETTO_BASEPOINT_TABLE, RISTRETTO_BASEPOINT_POINT},
  traits::Identity,
  scalar::Scalar,
  ristretto::{RistrettoPoint, CompressedRistretto}
};

use log::debug;

use crate::{DLEqError, DLEqResult, engines::{DLEqEngine, Commitment}};

lazy_static! {
  static ref ALT_BASEPOINT: RistrettoPoint = {
    CompressedRistretto(hex!("c6d77f893b5a01a5e995be5a568e55bb22f3931ee686f24e5d211bee967ec66d")).decompress().unwrap()
  };
}

#[derive(Clone, PartialEq)]
#[allow(non_snake_case)]
pub struct Signature {
  R: RistrettoPoint,
  s: Scalar
}

pub struct RistrettoEngine;
impl DLEqEngine for RistrettoEngine {
  type PrivateKey = Scalar;
  type PublicKey = RistrettoPoint;
  type Signature = Signature;

  fn alt_basepoint() -> Self::PublicKey {
    *ALT_BASEPOINT
  }

  fn scalar_bits() -> usize {
     252
  }

  fn new_private_key<R: RngCore + CryptoRng>(rng: &mut R) -> Self::PrivateKey {
    Scalar::random(rng)
  }

  fn to_public_key(key: &Self::PrivateKey) -> Self::PublicKey {
    key * &RISTRETTO_BASEPOINT_TABLE
  }

  fn little_endian_bytes_to_private_key(bytes: [u8; 32]) -> DLEqResult<Self::PrivateKey> {
    Scalar::from_canonical_bytes(bytes).ok_or(DLEqError::InvalidScalar)
  }

  fn private_key_to_little_endian_bytes(key: &Self::PrivateKey) -> [u8; 32] {
    key.to_bytes()
  }

  fn public_key_to_bytes(key: &Self::PublicKey) -> Vec<u8> {
    key.compress().to_bytes().to_vec()
  }

  fn bytes_to_public_key(key: &[u8]) -> DLEqResult<Self::PublicKey> {
    CompressedRistretto::from_slice(key).decompress().ok_or(DLEqError::InvalidPoint)
  }

  fn generate_commitments<R: RngCore + CryptoRng>(rng: &mut R, key: [u8; 32], bits: usize) -> Vec<Commitment<Self>> {
    let mut commitments = Vec::new();
    let mut blinding_key_total = Scalar::zero();
    let mut power_of_two = Scalar::one();
    let two = Scalar::from(2u8);
    for i in 0 .. bits {
      let blinding_key = if i == (bits - 1) {
        -blinding_key_total * power_of_two.invert()
      } else {
        Self::new_private_key(rng)
      };
      blinding_key_total += blinding_key * power_of_two;
      power_of_two *= two;

      let commitment_base = blinding_key * *ALT_BASEPOINT;
      let (commitment, commitment_minus_one) = if (key[i/8] >> (i % 8)) & 1 == 1 {
        (&commitment_base + &RISTRETTO_BASEPOINT_POINT, commitment_base)
      } else {
        (commitment_base, &commitment_base - &RISTRETTO_BASEPOINT_POINT)
      };

      commitments.push(Commitment {
        blinding_key,
        commitment_minus_one,
        commitment
      });
    }

    debug_assert_eq!(blinding_key_total, Scalar::zero());
    let pubkey = &Scalar::from_canonical_bytes(key).expect(
      "Generating commitments for an invalid Ristretto key"
    ) * &RISTRETTO_BASEPOINT_TABLE;
    debug_assert_eq!(
      &Self::reconstruct_key(commitments.iter().map(|c| &c.commitment)).expect("Reconstructed our key to invalid despite none being"),
      &pubkey
    );
    debug!("Generated DL Eq proof for Ristretto pubkey {}", hex::encode(pubkey.compress().as_bytes()));

    commitments
  }

  fn compute_signature_s(nonce: &Self::PrivateKey, challenge: [u8; 32], key: &Self::PrivateKey) -> Self::PrivateKey {
    nonce + Scalar::from_bytes_mod_order(challenge) * key
  }

  fn compute_signature_R(s_value: &Self::PrivateKey, challenge: [u8; 32], key: &Self::PublicKey) -> DLEqResult<Self::PublicKey> {
    Ok(s_value * *ALT_BASEPOINT - Scalar::from_bytes_mod_order(challenge) * key)
  }

  fn commitment_sub_one(commitment: &Self::PublicKey) -> DLEqResult<Self::PublicKey> {
    Ok(commitment - RISTRETTO_BASEPOINT_POINT)
  }

  fn reconstruct_key<'a>(commitments: impl Iterator<Item = &'a Self::PublicKey>) -> DLEqResult<Self::PublicKey> {
    let mut power_of_two = Scalar::one();
    let mut res = RistrettoPoint::identity();
    let two = Scalar::from(2u8);
    for comm in commitments {
      res += comm * power_of_two;
      power_of_two *= two;
    }
    Ok(res)
  }

  fn blinding_key_to_public(key: &Self::PrivateKey) -> Self::PublicKey {
    key * *ALT_BASEPOINT
  }

  fn sign(key: &Self::PrivateKey, message: &[u8]) -> Self::Signature {
      let k = Scalar::from_hash(Blake2b::new().chain(key.to_bytes()).chain(message));
      #[allow(non_snake_case)]
      let R = &RISTRETTO_BASEPOINT_POINT * k;

      let mut to_hash = R.compress().as_bytes().to_vec();
      to_hash.extend(message);
      let s = k - (*key * Scalar::from_bytes_mod_order(Blake2b::digest(&to_hash)[..32].try_into().unwrap()));

      Signature { R, s }
  }

  fn verify_signature(public_key: &Self::PublicKey, message: &[u8], signature: &Self::Signature) -> DLEqResult<()> {
    let mut to_hash = signature.R.compress().as_bytes().to_vec();
    to_hash.extend(message);
    let c = Scalar::from_bytes_mod_order(Blake2b::digest(&to_hash)[..32].try_into().unwrap());
    if RistrettoPoint::vartime_double_scalar_mul_basepoint(&c, &public_key, &signature.s) == signature.R {
      Ok(())
    } else {
      Err(DLEqError::InvalidSignature)
    }
  }

  fn point_len() -> usize {
    32
  }

  fn signature_len() -> usize {
    64
  }

  fn signature_to_bytes(sig: &Self::Signature) -> Vec<u8> {
    let mut res = Self::public_key_to_bytes(&sig.R);
    res.extend(sig.s.to_bytes());
    res
  }

  fn bytes_to_signature(sig: &[u8]) -> DLEqResult<Self::Signature> {
    if sig.len() != 64 {
      Err(DLEqError::InvalidSignature)
    } else {
      Ok(
        Self::Signature {
          R: Self::bytes_to_public_key(&sig[..32]).map_err(|_| DLEqError::InvalidSignature)?,
          s: Self::little_endian_bytes_to_private_key(sig[32..].try_into().expect(
            "Signature was correct length yet didn't have a 32-byte scalar")
          ).map_err(|_| DLEqError::InvalidSignature)?
        }
      )
    }
  }
}
