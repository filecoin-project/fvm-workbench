use cid::Cid;
use fvm_shared::{
    commcid::FIL_COMMITMENT_UNSEALED,
    crypto::signature::{SECP_PUB_LEN, SECP_SIG_LEN, SECP_SIG_MESSAGE_HASH_SIZE},
};
use libsecp256k1::{recover, Message, RecoveryId, Signature as EcsdaSignature};

use multihash::{derive::Multihash, MultihashDigest};

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

// multihash library doesn't support poseidon hashing, so we fake it
#[derive(Clone, Copy, Debug, PartialEq, Eq, Multihash)]
#[mh(alloc_size = 64)]
enum MhCode {
    #[mh(code = 0xb401, hasher = multihash::Sha2_256)]
    PoseidonFake,
    #[mh(code = 0x1012, hasher = multihash::Sha2_256)]
    Sha256TruncPaddedFake,
}

fn make_cid(input: &[u8], prefix: u64, hash: MhCode) -> Cid {
    let hash = hash.digest(input);
    Cid::new_v1(prefix, hash)
}

pub fn make_cid_sha(input: &[u8], prefix: u64) -> Cid {
    make_cid(input, prefix, MhCode::Sha256TruncPaddedFake)
}

pub fn make_cid_poseidon(input: &[u8], prefix: u64) -> Cid {
    make_cid(input, prefix, MhCode::PoseidonFake)
}

pub fn make_piece_cid(input: &[u8]) -> Cid {
    make_cid_sha(input, FIL_COMMITMENT_UNSEALED)
}
