use scale::{Decode, Encode};
use sp_core::Hasher as _;
use sp_runtime::DispatchError;

use crate::{
    runtime::{Contracts, ExecSideEffects, System, Timestamp},
    storage,
    types::{AccountId, BlockNumber, Hashing, ENOUGH, GAS_LIMIT},
};

use pallet_contracts_primitives::ContractExecResult;

pub type Storage = storage::Storage<storage::InMemoryBackend>;

#[derive(Debug)]
pub struct ExecError {
    pub source: DispatchError,
    pub message: String,
}

#[derive(Debug, Default, Encode, Decode)]
struct HookSelectors {
    on_block_end: Option<u32>,
}

#[derive(Debug, Encode, Decode)]
pub struct Contract {
    pub address: AccountId,
    hooks: HookSelectors,
}

impl Contract {
    pub fn new_storage() -> Storage {
        Storage::new(Default::default())
    }

    pub fn from_address(address: AccountId) -> Self {
        Contract {
            address,
            hooks: Default::default(),
        }
    }

    /// Create a new contract instance.
    ///
    /// # Parameters
    ///
    /// * `origin`: The owner of the created contract instance.
    /// * `code`: The contract code to deploy in raw bytes.
    /// * `input_data`: The input data to pass to the contract constructor.
    /// * `salt`: Used for the address derivation.
    pub fn new(
        storage: &mut Storage,
        origin: AccountId,
        code: Vec<u8>,
        input_data: Vec<u8>,
        salt: Vec<u8>,
        block_number: BlockNumber,
        now: u64,
    ) -> Result<(Self, ExecSideEffects), ExecError> {
        let code_hash = Hashing::hash(&code);

        let (address, effects) = storage.execute_with(false, move || -> Result<_, ExecError> {
            System::set_block_number(block_number);
            Timestamp::set_timestamp(now);

            let result = Contracts::bare_instantiate(
                origin.clone(),
                ENOUGH,
                GAS_LIMIT,
                pallet_contracts_primitives::Code::Upload(code.into()),
                input_data,
                salt.clone(),
                true,
            );
            match result.result {
                Err(err) => {
                    return Err(ExecError {
                        source: err,
                        message: String::from_utf8_lossy(&result.debug_message).to_string(),
                    });
                }
                Ok(_) => (),
            }
            Ok(Contracts::contract_address(&origin, &code_hash, &salt))
        });
        Ok((Self::from_address(address?), effects))
    }

    pub fn new_with_selector(
        storage: &mut Storage,
        origin: AccountId,
        code: Vec<u8>,
        selector: [u8; 4],
        args: impl Encode,
        salt: Vec<u8>,
        block_number: BlockNumber,
        now: u64,
    ) -> Result<(Self, ExecSideEffects), ExecError> {
        let mut input_data = vec![];
        selector.encode_to(&mut input_data);
        args.encode_to(&mut input_data);
        Self::new(storage, origin, code, input_data, salt, block_number, now)
    }

    /// Call a contract method
    ///
    /// # Parameters
    /// * `input_data`: The SCALE encoded arguments including the 4-bytes selector as prefix.
    /// # Return
    /// Returns the SCALE encoded method return value.
    pub fn bare_call(
        &mut self,
        storage: &mut Storage,
        origin: AccountId,
        input_data: Vec<u8>,
        rollback: bool,
        block_number: BlockNumber,
        now: u64,
    ) -> (ContractExecResult, ExecSideEffects) {
        let addr = self.address.clone();
        storage.execute_with(rollback, move || {
            System::set_block_number(block_number);
            Timestamp::set_timestamp(now);
            Contracts::bare_call(origin, addr, 0, GAS_LIMIT, input_data, true)
        })
    }

    /// Call a contract method given it's selector
    pub fn call_with_selector<RV: Decode>(
        &mut self,
        storage: &mut Storage,
        origin: AccountId,
        selector: [u8; 4],
        args: impl Encode,
        rollback: bool,
        block_number: BlockNumber,
        now: u64,
    ) -> Result<(RV, ExecSideEffects), ExecError> {
        let mut input_data = vec![];
        selector.encode_to(&mut input_data);
        args.encode_to(&mut input_data);
        let (result, effects) =
            self.bare_call(storage, origin, input_data, rollback, block_number, now);
        let mut rv = transpose_contract_result(&result)?;
        Ok((
            Decode::decode(&mut rv).or(Err(ExecError {
                source: DispatchError::Other("Decode result failed"),
                message: Default::default(),
            }))?,
            effects,
        ))
    }

    /// Called by on each block end by the runtime
    pub fn on_block_end(
        &mut self,
        storage: &mut Storage,
        block_number: BlockNumber,
        now: u64,
    ) -> Result<ExecSideEffects, ExecError> {
        if let Some(selector) = self.hooks.on_block_end {
            let mut input_data = vec![];
            selector.to_be_bytes().encode_to(&mut input_data);

            let (result, effects) = self.bare_call(
                storage,
                Default::default(),
                input_data,
                false,
                block_number,
                now,
            );
            let _ = transpose_contract_result(&result)?;
            Ok(effects)
        } else {
            Ok(Default::default())
        }
    }

    pub fn set_on_block_end_selector(&mut self, selector: u32) {
        self.hooks.on_block_end = Some(selector)
    }
}

pub fn transpose_contract_result(result: &ContractExecResult) -> Result<&[u8], ExecError> {
    result
        .result
        .as_ref()
        .map(|v| &*v.data.0)
        .map_err(|err| ExecError {
            source: err.clone(),
            message: String::from_utf8_lossy(&result.debug_message).to_string(),
        })
}

pub use contract_file::ContractFile;

mod contract_file {
    use impl_serde::serialize as bytes;
    use serde::Deserialize;
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ContractFile {
        pub metadata_version: String,
        pub source: Source,
        pub contract: Contract,
    }

    #[derive(Debug, Deserialize)]
    pub struct Source {
        #[serde(with = "bytes")]
        pub wasm: Vec<u8>,
        #[serde(with = "bytes")]
        pub hash: Vec<u8>,
        pub language: String,
        pub compiler: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct Contract {
        pub name: String,
        pub version: String,
    }

    impl ContractFile {
        pub fn load(json_contract: &[u8]) -> serde_json::Result<Self> {
            serde_json::from_slice(json_contract)
        }
    }
}
