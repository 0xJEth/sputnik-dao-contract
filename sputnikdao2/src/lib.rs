use near_contract_standards::fungible_token::metadata::{
    FungibleTokenMetadata, FungibleTokenMetadataProvider, FT_METADATA_SPEC,
};
use near_contract_standards::fungible_token::FungibleToken;
use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::collections::LookupMap;
#[cfg(target_arch = "wasm32")]
use near_sdk::env::BLOCKCHAIN_INTERFACE;
use near_sdk::json_types::{Base64VecU8, ValidAccountId, U128};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{env, near_bindgen, AccountId, Balance, PanicOnDefault, Promise, PromiseOrValue};

use crate::bounties::{Bounty, BountyClaim};
use crate::policy::Policy;
pub use crate::proposals::{Proposal, ProposalInput, ProposalKind};
pub use crate::types::{Action, Config};

mod bounties;
mod policy;
mod proposals;
mod types;
pub mod views;

near_sdk::setup_alloc!();

#[cfg(target_arch = "wasm32")]
const BLOCKCHAIN_INTERFACE_NOT_SET_ERR: &str = "Blockchain interface not set.";

#[near_bindgen]
#[derive(BorshSerialize, BorshDeserialize, PanicOnDefault)]
pub struct Contract {
    /// DAO configuration.
    config: Config,
    /// Voting and permissions policy.
    policy: Policy,
    /// Fungible token information and logic.
    token: FungibleToken,
    /// Last available id for the proposals.
    last_proposal_id: u64,
    /// Proposal map from ID to proposal information.
    proposals: LookupMap<u64, Proposal>,
    /// Last available id for the bounty.
    last_bounty_id: u64,
    /// Bounties map from ID to bounty information.
    bounties: LookupMap<u64, Bounty>,
    /// Bounty claimers map per user. Allows quickly to query for each users their claims.
    bounty_claimers: LookupMap<AccountId, Vec<BountyClaim>>,
    /// Count of claims per bounty.
    bounty_claims_count: LookupMap<u64, u32>,
    /// Large blob storage.
    blobs: LookupMap<Vec<u8>, AccountId>,
    /// Amount of $NEAR locked for storage / bonds.
    locked_amount: Balance,
}

#[near_bindgen]
impl Contract {
    #[init]
    pub fn new(config: Config, policy: Option<Policy>) -> Self {
        assert!(!env::state_exists(), "ERR_CONTRACT_IS_INITIALIZED");
        Self {
            config,
            policy: policy.unwrap_or_default(),
            token: FungibleToken::new(b"t".to_vec()),
            last_proposal_id: 0,
            proposals: LookupMap::new(b"p".to_vec()),
            last_bounty_id: 0,
            bounties: LookupMap::new(b"b".to_vec()),
            bounty_claimers: LookupMap::new(b"u".to_vec()),
            bounty_claims_count: LookupMap::new(b"c".to_vec()),
            blobs: LookupMap::new(b"d".to_vec()),
            // TODO: this doesn't account for this state object. Can just add fixed size of it.
            locked_amount: env::storage_byte_cost() * (env::storage_usage() as u128),
        }
    }

    /// Should only be called by this contract on migration.
    /// This is NOOP implementation. KEEP IT if you haven't changed contract state.
    /// If you have changed state, you need to implement migration from old state (keep the old struct with different name to deserialize it first).
    /// After migrate goes live on MainNet, return this implementation for next updates.
    #[init]
    pub fn migrate() -> Self {
        assert_eq!(
            env::predecessor_account_id(),
            env::current_account_id(),
            "ERR_NOT_ALLOWED"
        );
        let this: Contract = env::state_read().expect("ERR_CONTRACT_IS_NOT_INITIALIZED");
        this
    }

    /// Remove blob from contract storage and pay back to original storer.
    /// Only original storer can call this.
    pub fn remove_blob(&mut self, hash: Base64VecU8) -> Promise {
        let account_id = self.blobs.remove(&hash.0).expect("ERR_NO_BLOB");
        assert_eq!(
            env::predecessor_account_id(),
            account_id,
            "ERR_INVALID_CALLER"
        );
        env::storage_remove(&hash.0);
        let blob_len = env::register_len(u64::MAX - 1).unwrap();
        let storage_cost = ((blob_len + 32) as u128) * env::storage_byte_cost();
        self.locked_amount -= storage_cost;
        Promise::new(account_id).transfer(storage_cost)
    }
}

/// Optimal version for storing input data into storage.
/// Avoids parsing / loading into WASM the arguments.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn store_blob() {
    env::setup_panic_hook();
    env::set_blockchain_interface(Box::new(near_blockchain::NearBlockchain {}));
    let mut this: Contract = env::state_read().expect("ERR_CONTRACT_IS_NOT_INITIALIZED");
    unsafe {
        BLOCKCHAIN_INTERFACE.with(|b| {
            // Load input into register 0.
            b.borrow()
                .as_ref()
                .expect(BLOCKCHAIN_INTERFACE_NOT_SET_ERR)
                .input(0);
            // Compute sha256 hash of register 0 and store in 1.
            b.borrow()
                .as_ref()
                .expect(BLOCKCHAIN_INTERFACE_NOT_SET_ERR)
                .sha256(u64::MAX as _, 0 as _, 1);
            // Check if such blob already stored.
            assert_eq!(
                b.borrow()
                    .as_ref()
                    .expect(BLOCKCHAIN_INTERFACE_NOT_SET_ERR)
                    .storage_has_key(u64::MAX as _, 1 as _),
                0,
                "ERR_ALREADY_EXISTS"
            );
            // Get length of the input argument and check that enough $NEAR has been attached.
            let blob_len = b
                .borrow()
                .as_ref()
                .expect(BLOCKCHAIN_INTERFACE_NOT_SET_ERR)
                .register_len(0);
            let storage_cost = ((blob_len + 32) as u128) * env::storage_byte_cost();
            assert!(
                env::attached_deposit() >= storage_cost,
                "ERR_NOT_ENOUGH_DEPOSIT:{}",
                storage_cost
            );
            this.locked_amount += storage_cost;
            // Store value of register 0 into key = register 1.
            b.borrow()
                .as_ref()
                .expect(BLOCKCHAIN_INTERFACE_NOT_SET_ERR)
                .storage_write(u64::MAX as _, 1 as _, u64::MAX as _, 0 as _, 2);
            // Load register 1 into blob_hash and save into LookupMap.
            let blob_hash = vec![0u8; 32];
            b.borrow()
                .as_ref()
                .expect(BLOCKCHAIN_INTERFACE_NOT_SET_ERR)
                .read_register(1, blob_hash.as_ptr() as _);
            this.blobs
                .insert(&blob_hash, &env::predecessor_account_id());
            // Return from function value of register 1.
            let blob_hash_str = near_sdk::serde_json::to_string(&Base64VecU8(blob_hash))
                .unwrap()
                .into_bytes();
            b.borrow()
                .as_ref()
                .expect(BLOCKCHAIN_INTERFACE_NOT_SET_ERR)
                .value_return(blob_hash_str.len() as _, blob_hash_str.as_ptr() as _);
        });
    }
    env::state_write(&this);
}

near_contract_standards::impl_fungible_token_core!(Contract, token);
near_contract_standards::impl_fungible_token_storage!(Contract, token);

impl FungibleTokenMetadataProvider for Contract {
    fn ft_metadata(&self) -> FungibleTokenMetadata {
        FungibleTokenMetadata {
            spec: FT_METADATA_SPEC.to_string(),
            name: self.config.name.clone(),
            symbol: self.config.symbol.clone(),
            icon: self.config.icon.clone(),
            reference: self.config.reference.clone(),
            reference_hash: self.config.reference_hash.clone(),
            decimals: self.config.decimals,
        }
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::test_utils::{accounts, VMContextBuilder};
    use near_sdk::{testing_env, MockedBlockchain};
    use near_sdk_sim::to_yocto;

    use crate::proposals::ProposalStatus;
    use crate::types::BASE_TOKEN;

    use super::*;

    fn create_proposal(context: &mut VMContextBuilder, contract: &mut Contract) -> u64 {
        testing_env!(context.attached_deposit(to_yocto("1")).build());
        contract.add_proposal(ProposalInput {
            description: "test".to_string(),
            kind: ProposalKind::Transfer {
                token_id: BASE_TOKEN.to_string(),
                receiver_id: accounts(2).into(),
                amount: to_yocto("100"),
            },
        })
    }

    #[test]
    fn test_basics() {
        let mut context = VMContextBuilder::new();
        testing_env!(context.predecessor_account_id(accounts(1)).build());
        let mut contract = Contract::new(Config::test_config(), None);
        let id = create_proposal(&mut context, &mut contract);
        assert_eq!(contract.get_proposal(id).description, "test");
        contract.act_proposal(id, Action::RemoveProposal);
        assert_eq!(contract.get_proposals(0, 10).len(), 0);

        let id = create_proposal(&mut context, &mut contract);
        contract.act_proposal(id, Action::VoteApprove);
        assert_eq!(contract.get_proposal(id).status, ProposalStatus::Approved);

        let id = create_proposal(&mut context, &mut contract);
        // proposal expired, finalize.
        testing_env!(context
            .block_timestamp(1_000_000_000 * 24 * 60 * 60 * 8)
            .build());
        contract.act_proposal(id, Action::Finalize);
        assert_eq!(contract.get_proposal(id).status, ProposalStatus::Expired);
    }

    #[test]
    fn test_vote_expired_proposal() {
        let mut context = VMContextBuilder::new();
        testing_env!(context.predecessor_account_id(accounts(1)).build());
        let mut contract = Contract::new(Config::test_config(), None);
        let id = create_proposal(&mut context, &mut contract);
        testing_env!(context
            .block_timestamp(1_000_000_000 * 24 * 60 * 60 * 8)
            .build());
        contract.act_proposal(id, Action::VoteApprove);
    }
}
