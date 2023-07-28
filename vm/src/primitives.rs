use anyhow::anyhow;
use cid::Cid;
use fvm_shared::{
    address::Address,
    crypto::{
        hash::SupportedHashes,
        signature::{Signature, SECP_PUB_LEN, SECP_SIG_LEN, SECP_SIG_MESSAGE_HASH_SIZE},
    },
    piece::PieceInfo,
    sector::RegisteredSealProof,
};
use libsecp256k1::{recover, Message, RecoveryId, Signature as EcsdaSignature};
use multihash::{Code, MultihashDigest};
use vm_api::Primitives;

use crate::bench::kernel::make_piece_cid;

// Fake implementation of runtime primitives.
// Struct members can be added here to provide configurable functionality.
pub struct FakePrimitives {}

impl Primitives for FakePrimitives {
    fn hash_blake2b(&self, data: &[u8]) -> [u8; 32] {
        blake2b_simd::Params::new()
            .hash_length(32)
            .to_state()
            .update(data)
            .finalize()
            .as_bytes()
            .try_into()
            .unwrap()
    }

    fn hash(&self, hasher: SupportedHashes, data: &[u8]) -> Vec<u8> {
        let hasher = Code::try_from(hasher as u64).unwrap(); // supported hashes are all implemented in multihash
        hasher.digest(data).digest().to_owned()
    }

    fn hash_64(&self, hasher: SupportedHashes, data: &[u8]) -> ([u8; 64], usize) {
        let hasher = Code::try_from(hasher as u64).unwrap();
        let (len, buf, ..) = hasher.digest(data).into_inner();
        (buf, len as usize)
    }

    fn compute_unsealed_sector_cid(
        &self,
        _proof_type: RegisteredSealProof,
        _pieces: &[PieceInfo],
    ) -> Result<Cid, anyhow::Error> {
        Ok(make_piece_cid(b"test data"))
    }

    fn verify_signature(
        &self,
        signature: &Signature,
        _signer: &Address,
        plaintext: &[u8],
    ) -> Result<(), anyhow::Error> {
        if signature.bytes != plaintext {
            return Err(anyhow::format_err!(
                "invalid signature (mock sig validation expects siggy bytes to be equal to plaintext)"
            ));
        }
        Ok(())
    }

    fn recover_secp_public_key(
        &self,
        hash: &[u8; SECP_SIG_MESSAGE_HASH_SIZE],
        signature: &[u8; SECP_SIG_LEN],
    ) -> Result<[u8; SECP_PUB_LEN], anyhow::Error> {
        recover_secp_public_key(hash, signature)
            .map_err(|_| anyhow!("failed to recover secp public key"))
    }
}

#[allow(clippy::result_unit_err)]
pub fn recover_secp_public_key(
    hash: &[u8; SECP_SIG_MESSAGE_HASH_SIZE],
    signature: &[u8; SECP_SIG_LEN],
) -> Result<[u8; SECP_PUB_LEN], ()> {
    // generate types to recover key from
    let rec_id = RecoveryId::parse(signature[64]).map_err(|_| ())?;
    let message = Message::parse(hash);

    // Signature value without recovery byte
    let mut s = [0u8; 64];
    s.copy_from_slice(signature[..64].as_ref());

    // generate Signature
    let sig = EcsdaSignature::parse_standard(&s).map_err(|_| ())?;
    Ok(recover(&message, &sig, &rec_id).map_err(|_| ())?.serialize())
}
