use cid::Cid;
use fvm_shared::ActorID;
use fvm_shared::address::Address;
use fvm_shared::econ::TokenAmount;

/// A specification for installing built-in actors to seed a VM.
pub struct GenesisSpec {
    pub system_manifest_cid: Cid,
    pub reward_balance: TokenAmount,
    pub faucet_balance: TokenAmount,
    pub verifreg_signer: Address,
}

pub struct GenesisResult {
    pub verifreg_signer_id: ActorID,
    pub verifreg_root_id: ActorID,
    pub faucet_id: ActorID,
}

impl GenesisResult {
    pub fn verifreg_signer_address(&self) -> Address {
        Address::new_id(self.verifreg_signer_id)
    }
    pub fn verifreg_root_address(&self) -> Address {
        Address::new_id(self.verifreg_root_id)
    }
    pub fn faucet_address(&self) -> Address {
        Address::new_id(self.faucet_id)
    }
}