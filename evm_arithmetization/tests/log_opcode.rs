use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use bytes::Bytes;
use ethereum_types::{Address, BigEndianHash, H256};
use evm_arithmetization::generation::mpt::transaction_testing::{
    AddressOption, LegacyTransactionRlp,
};
use evm_arithmetization::generation::mpt::{AccountRlp, LegacyReceiptRlp, LogRlp};
use evm_arithmetization::generation::{GenerationInputs, TrieInputs};
use evm_arithmetization::proof::{BlockHashes, BlockMetadata, TrieRoots};
use evm_arithmetization::prover::testing::prove_all_segments;
use evm_arithmetization::testing_utils::{
    beacon_roots_account_nibbles, beacon_roots_contract_from_storage, ger_account_nibbles,
    init_logger, preinitialized_state_and_storage_tries, update_beacon_roots_account_storage,
    GLOBAL_EXIT_ROOT_ACCOUNT,
};
use evm_arithmetization::verifier::testing::verify_all_proofs;
use evm_arithmetization::{AllStark, Node, StarkConfig};
use hex_literal::hex;
use keccak_hash::keccak;
use mpt_trie::nibbles::Nibbles;
use mpt_trie::partial_trie::{HashedPartialTrie, PartialTrie};
use plonky2::field::goldilocks_field::GoldilocksField;
use plonky2::plonk::config::PoseidonGoldilocksConfig;
use plonky2::util::timing::TimingTree;

type F = GoldilocksField;
const D: usize = 2;
type C = PoseidonGoldilocksConfig;

/// Variation of `add11_yml` testing LOG opcodes.
#[test]
fn test_log_opcodes() -> anyhow::Result<()> {
    init_logger();

    let all_stark = AllStark::<F, D>::default();
    let config = StarkConfig::standard_fast_config();

    let beneficiary = hex!("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba");
    let sender = hex!("af1276cbb260bb13deddb4209ae99ae6e497f446");
    // Private key: DCDFF53B4F013DBCDC717F89FE3BF4D8B10512AAE282B48E01D7530470382701
    let to = hex!("095e7baea6a6c7c4c2dfeb977efac326af552d87");

    let beneficiary_state_key = keccak(beneficiary);
    let sender_state_key = keccak(sender);
    let to_hashed = keccak(to);

    let beneficiary_nibbles = Nibbles::from_bytes_be(beneficiary_state_key.as_bytes()).unwrap();
    let sender_nibbles = Nibbles::from_bytes_be(sender_state_key.as_bytes()).unwrap();
    let to_nibbles = Nibbles::from_bytes_be(to_hashed.as_bytes()).unwrap();

    // For the first code transaction code, we consider two LOG opcodes. The first
    // deals with 0 topics and empty data. The second deals with two topics, and
    // data of length 5, stored in memory.
    let code = [
        0x64, 0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0x60, 0x0, 0x52, // MSTORE(0x0, 0xA1B2C3D4E5)
        0x60, 0x0, 0x60, 0x0, 0xA0, // LOG0(0x0, 0x0)
        0x60, 99, 0x60, 98, 0x60, 5, 0x60, 27, 0xA2, // LOG2(27, 5, 98, 99)
        0x00,
    ];

    let code_gas = 3 + 3 + 3 // PUSHs and MSTORE
                 + 3 + 3 + 375 // PUSHs and LOG0
                 + 3 + 3 + 3 + 3 + 375 + 375*2 + 8*5 + 3// PUSHs, LOG2 and memory expansion
    ;
    let gas_used = 21_000 + code_gas;

    let code_hash = keccak(code);

    // Set accounts before the transaction.
    let beneficiary_account_before = AccountRlp {
        nonce: 1.into(),
        ..AccountRlp::default()
    };

    let sender_balance_before = 5000000000000000u64;
    let sender_account_before = AccountRlp {
        balance: sender_balance_before.into(),
        ..AccountRlp::default()
    };
    let to_account_before = AccountRlp {
        balance: 9000000000u64.into(),
        code_hash,
        ..AccountRlp::default()
    };

    // Initialize the state trie with three accounts.
    let (mut state_trie_before, mut storage_tries) = preinitialized_state_and_storage_tries()?;
    let mut beacon_roots_account_storage = storage_tries[0].1.clone();
    state_trie_before.insert(
        beneficiary_nibbles,
        rlp::encode(&beneficiary_account_before).to_vec(),
    )?;
    state_trie_before.insert(sender_nibbles, rlp::encode(&sender_account_before).to_vec())?;
    state_trie_before.insert(to_nibbles, rlp::encode(&to_account_before).to_vec())?;

    storage_tries.push((to_hashed, Node::Empty.into()));

    // We now add two receipts with logs and data. This updates the receipt trie as
    // well.
    let log_0 = LogRlp {
        address: hex!("7ef66b77759e12Caf3dDB3E4AFF524E577C59D8D").into(),
        topics: vec![
            hex!("8a22ee899102a366ac8ad0495127319cb1ff2403cfae855f83a89cda1266674d").into(),
            hex!("000000000000000000000000000000000000000000000000000000000000002a").into(),
            hex!("0000000000000000000000000000000000000000000000000000000000bd9fe6").into(),
        ],
        data: hex!("f7af1cc94b1aef2e0fa15f1b4baefa86eb60e78fa4bd082372a0a446d197fb58")
            .to_vec()
            .into(),
    };

    let receipt_0 = LegacyReceiptRlp {
            status: true,
            cum_gas_used: 0x016e5bu64.into(),
            bloom: hex!("00000000000000000000000000000000000000000000000000800000000000000040000000005000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000000000000000000080008000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000500000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000020000000000008000000000000000000000000").to_vec().into(),
            logs: vec![log_0],
        };

    // Insert the first receipt into the initial receipt trie. The initial receipts
    // trie has an initial node with a random nibble.
    let mut receipts_trie = HashedPartialTrie::from(Node::Empty);
    receipts_trie.insert(
        Nibbles::from_str("0x1337").unwrap(),
        rlp::encode(&receipt_0).to_vec(),
    )?;

    let tries_before = TrieInputs {
        state_trie: state_trie_before,
        transactions_trie: Node::Empty.into(),
        receipts_trie: receipts_trie.clone(),
        storage_tries,
    };

    // Prove a transaction which carries out two LOG opcodes.
    let txn_gas_price = 10;
    let txn = hex!("f860800a830186a094095e7baea6a6c7c4c2dfeb977efac326af552d87808026a0c3040cb042c541f9440771879b6bbf3f91464b265431de87eea1ec3206350eb8a046f5f3d06b8816f19f24ee919fd84bfb736db71df10a72fba4495f479e96f678");

    let block_metadata = BlockMetadata {
        block_beneficiary: Address::from(beneficiary),
        block_timestamp: 0x03e8.into(),
        block_number: 1.into(),
        block_difficulty: 0x020000.into(),
        block_random: H256::from_uint(&0x020000.into()),
        block_gaslimit: 0xffffffffu32.into(),
        block_chain_id: 1.into(),
        block_base_fee: 0xa.into(),
        ..Default::default()
    };

    let mut contract_code = HashMap::new();
    contract_code.insert(keccak(vec![]), vec![]);
    contract_code.insert(code_hash, code.to_vec());

    // Update the state and receipt tries after the transaction, so that we have the
    // correct expected tries: Update accounts
    #[cfg(feature = "cdk_erigon")]
    let beneficiary_account_after = AccountRlp {
        nonce: 1.into(),
        balance: block_metadata.block_base_fee * gas_used,
        ..AccountRlp::default()
    };
    #[cfg(not(feature = "cdk_erigon"))]
    let beneficiary_account_after = AccountRlp {
        nonce: 1.into(),
        ..AccountRlp::default()
    };

    let sender_balance_after = sender_balance_before - gas_used * txn_gas_price;
    let sender_account_after = AccountRlp {
        balance: sender_balance_after.into(),
        nonce: 1.into(),
        ..AccountRlp::default()
    };
    let to_account_after = AccountRlp {
        balance: 9000000000u64.into(),
        code_hash,
        ..AccountRlp::default()
    };

    update_beacon_roots_account_storage(
        &mut beacon_roots_account_storage,
        block_metadata.block_timestamp,
        block_metadata.parent_beacon_block_root,
    )?;
    let beacon_roots_account = beacon_roots_contract_from_storage(&beacon_roots_account_storage);

    // Update the receipt trie.
    let first_log = LogRlp {
        address: to.into(),
        topics: vec![],
        data: Bytes::new(),
    };

    let second_log = LogRlp {
        address: to.into(),
        topics: vec![
            hex!("0000000000000000000000000000000000000000000000000000000000000062").into(), /* dec: 98 */
            hex!("0000000000000000000000000000000000000000000000000000000000000063").into(), /* dec: 99 */
        ],
        data: hex!("a1b2c3d4e5").to_vec().into(),
    };

    let receipt = LegacyReceiptRlp {
        status: true,
        cum_gas_used: gas_used.into(),
        bloom: hex!("00000000000000001000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000000000000000000004000000000000000000000000000000800000000000000000000000000000000000000000000000000000000000000000000000000400000000000040000000000000000000000000002000000000000000000000000000").to_vec().into(),
        logs: vec![first_log, second_log],
    };

    let receipt_nibbles = Nibbles::from_str("0x80").unwrap(); // RLP(0) = 0x80

    receipts_trie.insert(receipt_nibbles, rlp::encode(&receipt).to_vec())?;

    // Update the state trie.
    let mut expected_state_trie_after = HashedPartialTrie::from(Node::Empty);
    expected_state_trie_after.insert(
        beneficiary_nibbles,
        rlp::encode(&beneficiary_account_after).to_vec(),
    )?;
    expected_state_trie_after
        .insert(sender_nibbles, rlp::encode(&sender_account_after).to_vec())?;
    expected_state_trie_after.insert(to_nibbles, rlp::encode(&to_account_after).to_vec())?;
    expected_state_trie_after.insert(
        beacon_roots_account_nibbles(),
        rlp::encode(&beacon_roots_account).to_vec(),
    )?;
    expected_state_trie_after.insert(
        ger_account_nibbles(),
        rlp::encode(&GLOBAL_EXIT_ROOT_ACCOUNT).to_vec(),
    )?;

    let transactions_trie: HashedPartialTrie = Node::Leaf {
        nibbles: Nibbles::from_str("0x80").unwrap(),
        value: txn.to_vec(),
    }
    .into();

    let trie_roots_after = TrieRoots {
        state_root: expected_state_trie_after.hash(),
        transactions_root: transactions_trie.hash(),
        receipts_root: receipts_trie.hash(),
    };

    let burn_addr = match cfg!(feature = "cdk_erigon") {
        true => Some(Address::from(beneficiary)),
        false => None,
    };

    let inputs = GenerationInputs {
        signed_txns: vec![txn.to_vec()],
        burn_addr,
        withdrawals: vec![],
        global_exit_roots: vec![],
        tries: tries_before,
        trie_roots_after,
        contract_code,
        checkpoint_state_trie_root: HashedPartialTrie::from(Node::Empty).hash(),
        block_metadata,
        txn_number_before: 0.into(),
        gas_used_before: 0.into(),
        gas_used_after: gas_used.into(),

        block_hashes: BlockHashes {
            prev_hashes: vec![H256::default(); 256],
            cur_hash: H256::default(),
        },
    };

    let max_cpu_len_log = 20;
    let mut timing = TimingTree::new("prove", log::Level::Debug);

    let proofs = prove_all_segments::<F, C, D>(
        &all_stark,
        &config,
        inputs,
        max_cpu_len_log,
        &mut timing,
        None,
    )?;

    timing.filter(Duration::from_millis(100)).print();

    verify_all_proofs(&all_stark, &proofs, &config)
}

/// Values taken from the block 1000000 of Goerli: https://goerli.etherscan.io/txs?block=1000000
#[test]
fn test_txn_and_receipt_trie_hash() -> anyhow::Result<()> {
    // This test checks that inserting into the transaction and receipt
    // `HashedPartialTrie`s works as expected.
    let mut example_txn_trie = HashedPartialTrie::from(Node::Empty);

    // We consider two transactions, with one log each.
    let transaction_0 = LegacyTransactionRlp {
        nonce: 157823u64.into(),
        gas_price: 1000000000u64.into(),
        gas: 250000u64.into(),
        to: AddressOption(Some(hex!("7ef66b77759e12Caf3dDB3E4AFF524E577C59D8D").into())),
        value: 0u64.into(),
        data: hex!("e9c6c176000000000000000000000000000000000000000000000000000000000000002a0000000000000000000000000000000000000000000000000000000000bd9fe6f7af1cc94b1aef2e0fa15f1b4baefa86eb60e78fa4bd082372a0a446d197fb58")
            .to_vec()
            .into(),
        v: 0x1c.into(),
        r: hex!("d0eeac4841caf7a894dd79e6e633efc2380553cdf8b786d1aa0b8a8dee0266f4").into(),
        s: hex!("740710eed9696c663510b7fb71a553112551121595a54ec6d2ec0afcec72a973").into(),
    };

    // Insert the first transaction into the transaction trie.
    example_txn_trie.insert(
        Nibbles::from_str("0x80").unwrap(), // RLP(0) = 0x80
        rlp::encode(&transaction_0).to_vec(),
    )?;

    let transaction_1 = LegacyTransactionRlp {
        nonce: 157824u64.into(),
        gas_price: 1000000000u64.into(),
        gas: 250000u64.into(),
        to: AddressOption(Some(hex!("7ef66b77759e12Caf3dDB3E4AFF524E577C59D8D").into())),
        value: 0u64.into(),
        data: hex!("e9c6c176000000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000004920eaa814f7df6a2203dc0e472e8828be95957c6b329fee8e2b1bb6f044c1eb4fc243")
            .to_vec()
            .into(),
        v: 0x1b.into(),
        r: hex!("a3ff39967683fc684dc7b857d6f62723e78804a14b091a058ad95cc1b8a0281f").into(),
        s: hex!("51b156e05f21f499fa1ae47ebf536b15a237208f1d4a62e33956b6b03cf47742").into(),
    };

    // Insert the second transaction into the transaction trie.
    example_txn_trie.insert(
        Nibbles::from_str("0x01").unwrap(),
        rlp::encode(&transaction_1).to_vec(),
    )?;

    // Receipts:
    let mut example_receipt_trie = HashedPartialTrie::from(Node::Empty);

    let log_0 = LogRlp {
        address: hex!("7ef66b77759e12Caf3dDB3E4AFF524E577C59D8D").into(),
        topics: vec![
            hex!("8a22ee899102a366ac8ad0495127319cb1ff2403cfae855f83a89cda1266674d").into(),
            hex!("000000000000000000000000000000000000000000000000000000000000002a").into(),
            hex!("0000000000000000000000000000000000000000000000000000000000bd9fe6").into(),
        ],
        data: hex!("f7af1cc94b1aef2e0fa15f1b4baefa86eb60e78fa4bd082372a0a446d197fb58")
            .to_vec()
            .into(),
    };

    let receipt_0 = LegacyReceiptRlp {
            status: true,
            cum_gas_used: 0x016e5bu64.into(),
            bloom: hex!("00000000000000000000000000000000000000000000000000800000000000000040000000005000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000000000000000000080008000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000500000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000020000000000008000000000000000000000000").to_vec().into(),
            logs: vec![log_0],
        };

    // Insert the first receipt into the receipt trie.
    example_receipt_trie.insert(
        Nibbles::from_str("0x80").unwrap(), // RLP(0) is 0x80
        rlp::encode(&receipt_0).to_vec(),
    )?;

    let log_1 = LogRlp {
        address: hex!("7ef66b77759e12Caf3dDB3E4AFF524E577C59D8D").into(),
        topics: vec![
            hex!("8a22ee899102a366ac8ad0495127319cb1ff2403cfae855f83a89cda1266674d").into(),
            hex!("0000000000000000000000000000000000000000000000000000000000000004").into(),
            hex!("00000000000000000000000000000000000000000000000000000000004920ea").into(),
        ],
        data: hex!("a814f7df6a2203dc0e472e8828be95957c6b329fee8e2b1bb6f044c1eb4fc243")
            .to_vec()
            .into(),
    };

    let receipt_1 = LegacyReceiptRlp {
            status: true,
            cum_gas_used: 0x02dcb6u64.into(),
            bloom: hex!("00000000000000000000000000000000000000000000000000800000000000000040000000001000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000008000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001000000400000000000000000000000000000002000040000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000008000000000000000000000000").to_vec().into(),
            logs: vec![log_1],
        };

    // Insert the second receipt into the receipt trie.
    example_receipt_trie.insert(
        Nibbles::from_str("0x01").unwrap(),
        rlp::encode(&receipt_1).to_vec(),
    )?;

    // Check that the trie hashes are correct.
    assert_eq!(
        example_txn_trie.hash(),
        hex!("3ab7120d12e1fc07303508542602beb7eecfe8f262b83fd71eefe7d6205242ce").into()
    );

    assert_eq!(
        example_receipt_trie.hash(),
        hex!("da46cdd329bfedace32da95f2b344d314bc6f55f027d65f9f4ac04ee425e1f98").into()
    );

    Ok(())
}
