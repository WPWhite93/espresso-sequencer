use std::sync::Arc;

use crate::{BlockBuildingSnafu, NodeState, Transaction};
use committable::{Commitment, Committable};
use hotshot_query_service::availability::QueryablePayload;
use hotshot_types::traits::{states::InstanceState, BlockPayload};
use hotshot_types::utils::BuilderCommitment;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use snafu::OptionExt;

pub mod entry;
pub mod payload;
pub mod queryable;
pub mod tables;
pub mod tx_iterator;

use entry::TxTableEntryWord;
use payload::Payload;
use tables::NameSpaceTable;

pub type NsTable = NameSpaceTable<TxTableEntryWord>;

impl BlockPayload for Payload<TxTableEntryWord> {
    type Error = crate::Error;
    type Transaction = Transaction;
    type Metadata = NsTable;

    /// Returns (Self, metadata).
    ///
    /// `metadata` is a bytes representation of the namespace table.
    /// Why bytes? To make it easy to move metadata into payload in the future.
    ///
    /// Namespace table defined as follows for j>0:
    /// word[0]:    [number of entries in namespace table]
    /// word[2j-1]: [id for the jth namespace]
    /// word[2j]:   [end byte index of the jth namespace in the payload]
    ///
    /// Thus, for j>2 the jth namespace payload bytes range is word[2(j-1)]..word[2j].
    /// Edge case: for j=1 the jth namespace start index is implicitly 0.
    ///
    /// Word type is `TxTableEntry`.
    /// TODO(746) don't use `TxTableEntry`; make a different type for type safety.
    ///
    /// TODO final entry should be implicit:
    /// https://github.com/EspressoSystems/espresso-sequencer/issues/757
    ///
    /// TODO(746) refactor and make pretty "table" code for tx, namespace tables?
    fn from_transactions(
        txs: impl IntoIterator<Item = Self::Transaction>,
        _state: Arc<dyn InstanceState>,
    ) -> Result<(Self, Self::Metadata), Self::Error> {
        let payload = Payload::from_txs(txs)?;
        let ns_table = payload.get_ns_table().clone(); // TODO don't clone ns_table
        Some((payload, ns_table)).context(BlockBuildingSnafu)
    }

    fn from_bytes(encoded_transactions: &[u8], metadata: &Self::Metadata) -> Self {
        Self {
            raw_payload: encoded_transactions.to_vec(),
            ns_table: metadata.clone(), // TODO don't clone ns_table
        }
    }

    // TODO remove
    fn genesis() -> (Self, Self::Metadata) {
        // this is only called from `Leaf::genesis`. Since we are
        // passing empty list, max_block_size is irrelevant so we can
        // use the mock NodeState. A future update to HotShot should
        // make a change there to remove the need for this workaround.

        Self::from_transactions([], Arc::new(NodeState::mock())).unwrap()
    }

    fn encode(&self) -> Result<Arc<[u8]>, Self::Error> {
        Ok(Arc::from(self.raw_payload.clone()))
    }

    fn transaction_commitments(&self, meta: &Self::Metadata) -> Vec<Commitment<Self::Transaction>> {
        self.enumerate(meta).map(|(_, tx)| tx.commit()).collect()
    }

    /// Generate commitment that builders use to sign block options.
    fn builder_commitment(&self, metadata: &Self::Metadata) -> BuilderCommitment {
        let mut digest = sha2::Sha256::new();
        digest.update((self.raw_payload.len() as u64).to_le_bytes());
        digest.update((self.ns_table.bytes.len() as u64).to_le_bytes());
        digest.update((metadata.bytes.len() as u64).to_le_bytes());
        digest.update(&self.raw_payload);
        digest.update(&self.ns_table.bytes);
        digest.update(&metadata.bytes);
        BuilderCommitment::from_raw_digest(digest.finalize())
    }

    fn get_transactions<'a>(
        &'a self,
        metadata: &'a Self::Metadata,
    ) -> impl 'a + Iterator<Item = Self::Transaction> {
        self.enumerate(metadata).map(|(_, t)| t)
    }
}

#[cfg(test)]
mod reference {
    //! Reference data types.
    //!
    //! This module provides some reference instantiations of various data types which have an
    //! external, language-independent interface (e.g. commitment scheme). Ports of the sequencer to
    //! other languages, as well as downstream packages written in other languages, can use these
    //! references objects and their known commitments to check that their implementations of the
    //! commitment scheme are compatible with this reference implementation. To get the byte
    //! representation or U256 representation of a commitment for testing in other packages, run the
    //! tests and look for "commitment bytes" or "commitment U256" in the logs.
    //!
    //! For convenience, the reference objects are provided in serialized form, as they will appear
    //! in query service responses and the like, in the JSON files in the `data` directory of the
    //! repo for this crate. These JSON files are compiled into the crate binary and deserialized in
    //! this module to generate tests for the serialization format and commitment scheme.
    //!
    //! These tests may fail if you make a breaking change to a commitment scheme, serialization,
    //! etc. If this happens, be sure you _want_ to break the API, and, if so, simply replace the
    //! relevant constant in this module with the "actual" value that can be found in the logs of
    //! the failing test.

    use super::*;
    use crate::{Header, L1BlockInfo};
    use async_compatibility_layer::logging::{setup_backtrace, setup_logging};
    use lazy_static::lazy_static;
    use sequencer_utils::commitment_to_u256;
    use serde::de::DeserializeOwned;
    use serde_json::Value;

    macro_rules! load_reference {
        ($name:expr) => {
            serde_json::from_str(include_str!(std::concat!("../../data/", $name, ".json"))).unwrap()
        };
    }

    lazy_static! {
        pub static ref NS_TABLE: Value = load_reference!("ns_table");
        pub static ref L1_BLOCK: Value = load_reference!("l1_block");
        pub static ref HEADER: Value = load_reference!("header");
        pub static ref TRANSACTION: Value = load_reference!("transaction");
    }

    fn reference_test<T: DeserializeOwned, C: Committable>(
        reference: Value,
        expected: &str,
        commit: impl FnOnce(&T) -> Commitment<C>,
    ) {
        setup_logging();
        setup_backtrace();

        let reference: T = serde_json::from_value(reference).unwrap();
        let actual = commit(&reference);

        // Print information about the commitment that might be useful in generating tests for other
        // languages.
        let bytes: &[u8] = actual.as_ref();
        let u256 = commitment_to_u256(actual);
        tracing::info!("actual commitment: {}", actual);
        tracing::info!("commitment bytes: {:?}", bytes);
        tracing::info!("commitment U256: {}", u256);

        assert_eq!(actual, expected.parse().unwrap());
    }

    #[test]
    fn test_reference_ns_table() {
        reference_test::<NameSpaceTable<TxTableEntryWord>, _>(
            NS_TABLE.clone(),
            "NSTABLE~GL-lEBAwNZDldxDpySRZQChNnmn9vNzdIAL8W9ENOuh_",
            |ns_table| ns_table.commit(),
        );
    }

    #[test]
    fn test_reference_l1_block() {
        reference_test::<L1BlockInfo, _>(
            L1_BLOCK.clone(),
            "L1BLOCK~4HpzluLK2Isz3RdPNvNrDAyQcWOF2c9JeLZzVNLmfpQ9",
            |block| block.commit(),
        );
    }

    #[test]
    fn test_reference_header() {
        reference_test::<Header, _>(
            HEADER.clone(),
            "BLOCK~00ISpu2jHbXD6z-BwMkwR4ijGdgUSoXLp_2jIStmqBrD",
            |header| header.commit(),
        );
    }

    #[test]
    fn test_reference_transaction() {
        reference_test::<Transaction, _>(
            TRANSACTION.clone(),
            "TX~77xOf9b3_RtGwqQ7_zOPeuJRS0iZwF7EJiV_NzOv4uJ3",
            |tx| tx.commit(),
        );
    }
}
