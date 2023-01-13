use fvm_shared::econ::TokenAmount;
use std::collections::HashMap;
use fvm_shared::{ActorID, BLOCK_GAS_LIMIT, MethodNum};
use fvm_shared::address::Address;
use fvm_ipld_encoding::{de, from_slice, RawBytes};
use fvm_shared::clock::ChainEpoch;
use anyhow::anyhow;
use fvm_shared::message::Message;
use fvm_shared::bigint::Zero;
use crate::{ActorState, Bench, ExecutionResult};

pub struct ExecutionWrangler<'a> {
    bench: &'a mut dyn Bench,
    version: i64,
    gas_limit: i64,
    gas_fee_cap: TokenAmount,
    gas_premium: TokenAmount,
    sequences: HashMap<Address, u64>,
    msg_length: usize,
    compute_msg_length: bool,
}

impl<'a> ExecutionWrangler<'a> {
    pub fn new(
        bench: &'a mut dyn Bench,
        version: i64,
        gas_limit: i64,
        gas_fee_cap: TokenAmount,
        gas_premium: TokenAmount,
        compute_msg_length: bool,
    ) -> Self {
        Self {
            bench,
            version,
            gas_limit,
            gas_fee_cap,
            gas_premium,
            sequences: HashMap::new(),
            msg_length: 0,
            compute_msg_length,
        }
    }

    pub fn new_default(bench: &'a mut dyn Bench) -> Self {
        Self::new(bench, 0, BLOCK_GAS_LIMIT, TokenAmount::zero(), TokenAmount::zero(), true)
    }

    pub fn execute(
        &mut self,
        from: Address,
        to: Address,
        method: MethodNum,
        params: RawBytes,
        value: TokenAmount,
    ) -> anyhow::Result<ExecutionResult> {
        let sequence = self.sequences.get(&from).unwrap_or(&0);
        let (msg, msg_length) = self.make_msg(from, to, method, params, value, *sequence);
        let ret = self.bench.execute(msg, msg_length);
        if ret.is_ok() {
            self.sequences.insert(from, sequence + 1);
        }
        ret
    }

    pub fn execute_implicit(
        &mut self,
        from: Address,
        to: Address,
        method: MethodNum,
        params: RawBytes,
        value: TokenAmount,
    ) -> anyhow::Result<ExecutionResult> {
        let sequence = self.sequences.get(&from).unwrap_or(&0);
        let (msg, msg_length) = self.make_msg(from, to, method, params, value, *sequence);
        let ret = self.bench.execute_implicit(msg, msg_length);
        if ret.is_ok() {
            self.sequences.insert(from, sequence + 1);
        }
        ret
    }

    pub fn epoch(&self) -> ChainEpoch {
        self.bench.epoch()
    }

    pub fn find_actor(&self, id: ActorID) -> anyhow::Result<Option<ActorState>> {
        self.bench.find_actor(id)
    }

    pub fn find_actor_state<T: de::DeserializeOwned>(
        &self,
        id: ActorID,
    ) -> anyhow::Result<Option<T>> {
        let actor = self.bench.find_actor(id)?;
        Ok(match actor {
            Some(actor) => {
                let block = self
                    .bench
                    .store()
                    .get(&actor.state)
                    .map_err(|e| anyhow!("failed to load state for actor {}: {}", id, e))?;

                block
                    .map(|s| {
                        from_slice(&s)
                            .map_err(|e| anyhow!("failed to deserialize actor state: {}", e))
                    })
                    .transpose()?
            }
            None => None,
        })
    }

    pub fn resolve_address(&self, addr: &Address) -> anyhow::Result<Option<ActorID>> {
        self.bench.resolve_address(addr)
    }

    ///// Private helpers /////

    fn make_msg(
        &self,
        from: Address,
        to: Address,
        method: MethodNum,
        params: RawBytes,
        value: TokenAmount,
        sequence: u64,
    ) -> (Message, usize) {
        let msg = Message {
            from,
            to,
            sequence,
            value,
            method_num: method,
            params,
            version: self.version,
            gas_limit: self.gas_limit,
            gas_fee_cap: self.gas_fee_cap.clone(),
            gas_premium: self.gas_premium.clone(),
        };
        let msg_length = if self.compute_msg_length {
            self.msg_length
        } else {
            0 // FIXME serialize and size
        };
        (msg, msg_length)
    }
}
