use phala_crypto::ecdh::EcdhPublicKey;
use phala_mq::traits::MessageChannel;
use runtime::BlockNumber;
use serde::{Deserialize, Serialize};

use super::pink::cluster::ClusterKeeper;
use super::*;
use crate::secret_channel::SecretReceiver;
use crate::types::BlockInfo;
use phala_serde_more as more;

pub struct ExecuteEnv<'a, 'b> {
    pub block: &'a mut BlockInfo<'b>,
    pub contract_clusters: &'a mut ClusterKeeper,
}

pub struct NativeContext<'a, 'b> {
    pub block: &'a mut BlockInfo<'b>,
    pub mq: &'a SignedMessageChannel,
    pub secret_mq: SecretMessageChannel<'a, SignedMessageChannel>,
    pub contract_clusters: &'a mut ClusterKeeper,
    pub self_id: ContractId,
}

pub struct QueryContext<'a> {
    pub block_number: BlockNumber,
    pub now_ms: u64,
    pub contract_clusters: &'a mut ClusterKeeper,
}

impl NativeContext<'_, '_> {
    pub fn mq(&self) -> &SignedMessageChannel {
        self.mq
    }
}

pub trait Contract {
    fn id(&self) -> ContractId;
    fn handle_query(
        &mut self,
        origin: Option<&chain::AccountId>,
        req: OpaqueQuery,
        context: &mut QueryContext,
    ) -> Result<OpaqueReply, OpaqueError>;
    fn cluster_id(&self) -> Option<phala_mq::ContractClusterId>;
    fn process_next_message(&mut self, env: &mut ExecuteEnv) -> Option<TransactionResult>;
    fn on_block_end(&mut self, env: &mut ExecuteEnv) -> TransactionResult;
    fn push_message(&self, payload: Vec<u8>, topic: Vec<u8>);
    fn push_osp_message(
        &self,
        payload: Vec<u8>,
        topic: Vec<u8>,
        remote_pubkey: Option<&EcdhPublicKey>,
    );
    fn set_on_block_end_selector(&mut self, selector: u32);
}

pub trait NativeContract {
    type Cmd: Decode + Debug;
    type QReq: Decode + Debug;
    type QResp: Encode + Debug;

    fn id(&self) -> ContractId;
    fn cluster_id(&self) -> Option<phala_mq::ContractClusterId> {
        None
    }
    fn handle_command(
        &mut self,
        _origin: MessageOrigin,
        _cmd: Self::Cmd,
        _context: &mut NativeContext,
    ) -> TransactionResult {
        Ok(Default::default())
    }
    fn handle_query(
        &mut self,
        origin: Option<&chain::AccountId>,
        req: Self::QReq,
        context: &mut QueryContext,
    ) -> Self::QResp;
    fn on_block_end(&mut self, _context: &mut NativeContext) -> TransactionResult {
        Ok(Default::default())
    }
    fn set_on_block_end_selector(&mut self, _selector: u32) {}
}

#[derive(Serialize, Deserialize)]
pub struct NativeCompatContract<Con: NativeContract> {
    #[serde(bound(
        serialize = "Con: Encode",
        deserialize = "Con: Decode"
    ))]
    #[serde(with = "more::scale_bytes")]
    contract: Con,
    send_mq: SignedMessageChannel,
    cmd_rcv_mq: SecretReceiver<Con::Cmd>,
    #[serde(with = "crate::secret_channel::ecdh_serde")]
    ecdh_key: KeyPair,
}

impl<Con: NativeContract> NativeCompatContract<Con> {
    pub fn new(
        contract: Con,
        send_mq: SignedMessageChannel,
        cmd_rcv_mq: SecretReceiver<Con::Cmd>,
        ecdh_key: KeyPair,
    ) -> Self {
        NativeCompatContract {
            contract,
            send_mq,
            cmd_rcv_mq,
            ecdh_key,
        }
    }
}

impl<Con: NativeContract> Contract for NativeCompatContract<Con> {
    fn id(&self) -> ContractId {
        self.contract.id()
    }

    fn cluster_id(&self) -> Option<phala_mq::ContractClusterId> {
        self.contract.cluster_id()
    }

    fn handle_query(
        &mut self,
        origin: Option<&runtime::AccountId>,
        req: OpaqueQuery,
        context: &mut QueryContext,
    ) -> Result<OpaqueReply, OpaqueError> {
        debug!(target: "contract", "Contract {:?} handling query", self.id());
        let response = self
            .contract
            .handle_query(origin, deopaque_query(req)?, context);
        Ok(response.encode())
    }

    fn process_next_message(&mut self, env: &mut ExecuteEnv) -> Option<TransactionResult> {
        let secret_mq = SecretMessageChannel::new(&self.ecdh_key, &self.send_mq);
        let mut context = NativeContext {
            block: env.block,
            mq: &self.send_mq,
            secret_mq,
            contract_clusters: &mut env.contract_clusters,
            self_id: self.id(),
        };

        phala_mq::select! {
            next_cmd = self.cmd_rcv_mq => match next_cmd {
                Ok((_, cmd, origin)) => {
                    info!(target: "contract", "Contract {:?} handling command", self.id());
                    self.contract.handle_command(origin, cmd, &mut context)
                }
                Err(_e) => {
                    Err(TransactionError::ChannelError)
                }
            },
        }
    }

    fn on_block_end(&mut self, env: &mut ExecuteEnv) -> TransactionResult {
        let secret_mq = SecretMessageChannel::new(&self.ecdh_key, &self.send_mq);
        let mut context = NativeContext {
            block: env.block,
            mq: &self.send_mq,
            secret_mq,
            contract_clusters: &mut env.contract_clusters,
            self_id: self.id(),
        };
        self.contract.on_block_end(&mut context)
    }

    fn set_on_block_end_selector(&mut self, selector: u32) {
        self.contract.set_on_block_end_selector(selector)
    }

    fn push_message(&self, payload: Vec<u8>, topic: Vec<u8>) {
        self.send_mq.push_data(payload, topic)
    }

    fn push_osp_message(
        &self,
        payload: Vec<u8>,
        topic: Vec<u8>,
        remote_pubkey: Option<&EcdhPublicKey>,
    ) {
        let secret_mq = SecretMessageChannel::new(&self.ecdh_key, &self.send_mq);
        secret_mq
            .bind_remote_key(remote_pubkey)
            .push_data(payload, topic)
    }
}

pub use keeper::*;
mod keeper;
