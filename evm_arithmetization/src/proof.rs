use ethereum_types::{Address, H256, U256};
use plonky2::field::extension::Extendable;
use plonky2::hash::hash_types::{HashOutTarget, MerkleCapTarget, RichField, NUM_HASH_OUT_ELTS};
use plonky2::iop::target::{BoolTarget, Target};
use plonky2::plonk::circuit_builder::CircuitBuilder;
use plonky2::plonk::config::GenericConfig;
use plonky2::util::serialization::{Buffer, IoResult, Read, Write};
use serde::{Deserialize, Serialize};
use starky::config::StarkConfig;
use starky::lookup::GrandProductChallengeSet;
use starky::proof::{MultiProof, StarkProofChallenges};

use crate::all_stark::NUM_TABLES;
use crate::util::{get_h160, get_h256, get_u256, h2u};
use crate::witness::state::RegistersState;

/// The default cap height used for our zkEVM STARK proofs.
pub(crate) const DEFAULT_CAP_HEIGHT: usize = 4;
/// Number of elements contained in a Merkle cap with default height.
pub(crate) const DEFAULT_CAP_LEN: usize = 1 << DEFAULT_CAP_HEIGHT;

/// A STARK proof for each table, plus some metadata used to create recursive
/// wrapper proofs.
#[derive(Debug, Clone)]
pub struct AllProof<F: RichField + Extendable<D>, C: GenericConfig<D, F = F>, const D: usize> {
    /// A multi-proof containing all proofs for the different STARK modules and
    /// their cross-table lookup challenges.
    pub multi_proof: MultiProof<F, C, D, NUM_TABLES>,
    /// Public memory values used for the recursive proofs.
    pub public_values: PublicValues,
}

impl<F: RichField + Extendable<D>, C: GenericConfig<D, F = F>, const D: usize> AllProof<F, C, D> {
    /// Returns the degree (i.e. the trace length) of each STARK.
    pub fn degree_bits(&self, config: &StarkConfig) -> [usize; NUM_TABLES] {
        self.multi_proof.recover_degree_bits(config)
    }
}

/// Randomness for all STARKs.
pub(crate) struct AllProofChallenges<F: RichField + Extendable<D>, const D: usize> {
    /// Randomness used in each STARK proof.
    pub stark_challenges: [StarkProofChallenges<F, D>; NUM_TABLES],
    /// Randomness used for cross-table lookups. It is shared by all STARKs.
    pub ctl_challenges: GrandProductChallengeSet<F>,
}

/// Memory values which are public.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct PublicValues {
    /// Trie hashes before the execution of the local state transition
    pub trie_roots_before: TrieRoots,
    /// Trie hashes after the execution of the local state transition.
    pub trie_roots_after: TrieRoots,
    /// Address to store the base fee to be burnt: only used when feature
    /// `cdk_erigon` is active.
    pub burn_addr: Option<U256>,
    /// Block metadata: it remains unchanged within a block.
    pub block_metadata: BlockMetadata,
    /// 256 previous block hashes and current block's hash.
    pub block_hashes: BlockHashes,
    /// Extra block data that is specific to the current proof.
    pub extra_block_data: ExtraBlockData,
    /// Registers to initialize the current proof.
    pub registers_before: RegistersData,
    /// Registers at the end of the current proof.
    pub registers_after: RegistersData,

    pub mem_before: MemCap,
    pub mem_after: MemCap,
}

impl PublicValues {
    /// Extracts public values from the given public inputs of a proof.
    /// Public values are always the first public inputs added to the circuit,
    /// so we can start extracting at index 0.
    /// `len_mem_cap` is the length of the `MemBefore` and `MemAfter` caps.
    pub fn from_public_inputs<F: RichField>(pis: &[F]) -> Self {
        assert!(pis.len() >= PublicValuesTarget::SIZE);

        let mut offset = 0;
        let trie_roots_before =
            TrieRoots::from_public_inputs(&pis[offset..offset + TrieRootsTarget::SIZE]);
        offset += TrieRootsTarget::SIZE;
        let trie_roots_after =
            TrieRoots::from_public_inputs(&pis[offset..offset + TrieRootsTarget::SIZE]);
        offset += TrieRootsTarget::SIZE;
        let burn_addr = match cfg!(feature = "cdk_erigon") {
            true => Some(get_u256(
                &pis[offset..offset + BurnAddrTarget::get_size()]
                    .try_into()
                    .unwrap(),
            )),
            false => None,
        };
        offset += BurnAddrTarget::get_size();
        let block_metadata =
            BlockMetadata::from_public_inputs(&pis[offset..offset + BlockMetadataTarget::SIZE]);
        offset += BlockMetadataTarget::SIZE;
        let block_hashes =
            BlockHashes::from_public_inputs(&pis[offset..offset + BlockHashesTarget::SIZE]);
        offset += BlockHashesTarget::SIZE;
        let extra_block_data =
            ExtraBlockData::from_public_inputs(&pis[offset..offset + ExtraBlockDataTarget::SIZE]);
        offset += ExtraBlockDataTarget::SIZE;
        let registers_before =
            RegistersData::from_public_inputs(&pis[offset..offset + RegistersDataTarget::SIZE]);
        offset += RegistersDataTarget::SIZE;
        let registers_after =
            RegistersData::from_public_inputs(&pis[offset..offset + RegistersDataTarget::SIZE]);
        offset += RegistersDataTarget::SIZE;
        let mem_before = MemCap::from_public_inputs(&pis[offset..offset + MemCapTarget::SIZE]);
        offset += MemCapTarget::SIZE;
        let mem_after = MemCap::from_public_inputs(&pis[offset..offset + MemCapTarget::SIZE]);

        Self {
            trie_roots_before,
            trie_roots_after,
            burn_addr,
            block_metadata,
            block_hashes,
            extra_block_data,
            registers_before,
            registers_after,
            mem_before,
            mem_after,
        }
    }
}

/// Memory values which are public once a final block proof is generated.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct FinalPublicValues {
    /// State trie root before the execution of this global state transition.
    pub state_trie_root_before: H256,
    /// State trie root after the execution of this global state transition.
    pub state_trie_root_after: H256,
}

impl FinalPublicValues {
    /// Extracts final public values from the given public inputs of a proof.
    /// Public values are always the first public inputs added to the circuit,
    /// so we can start extracting at index 0.
    pub fn from_public_inputs<F: RichField>(pis: &[F]) -> Self {
        assert!(FinalPublicValuesTarget::SIZE <= pis.len());

        let mut offset = 0;
        let state_trie_root_before = get_h256(&pis[offset..offset + TARGET_HASH_SIZE]);
        offset += TARGET_HASH_SIZE;
        let state_trie_root_after = get_h256(&pis[offset..offset + TARGET_HASH_SIZE]);

        Self {
            state_trie_root_before,
            state_trie_root_after,
        }
    }
}

impl From<PublicValues> for FinalPublicValues {
    fn from(value: PublicValues) -> Self {
        Self {
            state_trie_root_before: value.trie_roots_before.state_root,
            state_trie_root_after: value.trie_roots_after.state_root,
        }
    }
}

/// Memory values which are public once a final block proof is generated.
/// Note: All the larger integers are encoded with 32-bit limbs in little-endian
/// order.
#[derive(Eq, PartialEq, Debug)]
pub struct FinalPublicValuesTarget {
    /// State trie root before the execution of this global state transition.
    pub state_trie_root_before: [Target; TARGET_HASH_SIZE],
    /// State trie root after the execution of this global state transition.
    pub state_trie_root_after: [Target; TARGET_HASH_SIZE],
}

impl FinalPublicValuesTarget {
    pub(crate) const SIZE: usize = TARGET_HASH_SIZE * 2;

    /// Serializes public value targets.
    pub(crate) fn to_buffer(&self, buffer: &mut Vec<u8>) -> IoResult<()> {
        buffer.write_target_array(&self.state_trie_root_before)?;
        buffer.write_target_array(&self.state_trie_root_after)?;

        Ok(())
    }

    /// Deserializes public value targets.
    pub(crate) fn from_buffer(buffer: &mut Buffer) -> IoResult<Self> {
        let state_trie_root_before = buffer.read_target_array()?;
        let state_trie_root_after = buffer.read_target_array()?;

        Ok(Self {
            state_trie_root_before,
            state_trie_root_after,
        })
    }

    /// Connects these `FinalPublicValuesTarget` with their corresponding
    /// counterpart in a full parent `PublicValuesTarget`.
    pub(crate) fn connect_parent<F: RichField + Extendable<D>, const D: usize>(
        &self,
        builder: &mut CircuitBuilder<F, D>,
        pv1: &PublicValuesTarget,
    ) {
        for i in 0..8 {
            builder.connect(
                self.state_trie_root_before[i],
                pv1.trie_roots_before.state_root[i],
            );
            builder.connect(
                self.state_trie_root_after[i],
                pv1.trie_roots_after.state_root[i],
            );
            // We only use `FinalPublicValues` at the final block proof wrapping stage,
            // where we should enforce consistency with the known checkpoint.
            builder.connect(
                self.state_trie_root_before[i],
                pv1.extra_block_data.checkpoint_state_trie_root[i],
            );
        }
    }
}

/// Trie hashes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrieRoots {
    /// State trie hash.
    pub state_root: H256,
    /// Transaction trie hash.
    pub transactions_root: H256,
    /// Receipts trie hash.
    pub receipts_root: H256,
}

impl TrieRoots {
    pub fn from_public_inputs<F: RichField>(pis: &[F]) -> Self {
        assert!(pis.len() == TrieRootsTarget::SIZE);

        let state_root = get_h256(&pis[0..TARGET_HASH_SIZE]);
        let transactions_root = get_h256(&pis[TARGET_HASH_SIZE..2 * TARGET_HASH_SIZE]);
        let receipts_root = get_h256(&pis[2 * TARGET_HASH_SIZE..3 * TARGET_HASH_SIZE]);

        Self {
            state_root,
            transactions_root,
            receipts_root,
        }
    }
}

// There should be 256 previous hashes stored, so the default should also
// contain 256 values.
impl Default for BlockHashes {
    fn default() -> Self {
        Self {
            prev_hashes: vec![H256::default(); 256],
            cur_hash: H256::default(),
        }
    }
}

/// User-provided helper values to compute the `BLOCKHASH` opcode.
/// The proofs across consecutive blocks ensure that these values
/// are consistent (i.e. shifted by one to the left).
///
/// When the block number is less than 256, dummy values, i.e.
/// `H256::default()`, should be used for the additional block hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHashes {
    /// The previous 256 hashes to the current block. The leftmost hash, i.e.
    /// `prev_hashes[0]`, is the oldest, and the rightmost, i.e.
    /// `prev_hashes[255]` is the hash of the parent block.
    pub prev_hashes: Vec<H256>,
    // The hash of the current block.
    pub cur_hash: H256,
}

impl BlockHashes {
    pub fn from_public_inputs<F: RichField>(pis: &[F]) -> Self {
        assert!(pis.len() == BlockHashesTarget::SIZE);

        let prev_hashes: [H256; 256] = core::array::from_fn(|i| {
            get_h256(&pis[TARGET_HASH_SIZE * i..TARGET_HASH_SIZE * (i + 1)])
        });
        let cur_hash = get_h256(&pis[2048..2056]);

        Self {
            prev_hashes: prev_hashes.to_vec(),
            cur_hash,
        }
    }
}

/// Metadata contained in a block header. Those are identical between
/// all state transition proofs within the same block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct BlockMetadata {
    /// The address of this block's producer.
    pub block_beneficiary: Address,
    /// The timestamp of this block.
    pub block_timestamp: U256,
    /// The index of this block.
    pub block_number: U256,
    /// The difficulty (before PoS transition) of this block.
    pub block_difficulty: U256,
    pub block_random: H256,
    /// The gas limit of this block. It must fit in a `u32`.
    pub block_gaslimit: U256,
    /// The chain id of this block.
    pub block_chain_id: U256,
    /// The base fee of this block.
    pub block_base_fee: U256,
    /// The total gas used in this block. It must fit in a `u32`.
    pub block_gas_used: U256,
    /// The blob gas used. It must fit in a `u64`.
    pub block_blob_gas_used: U256,
    /// The excess blob base. It must fit in a `u64`.
    pub block_excess_blob_gas: U256,
    /// The hash tree root of the parent beacon block.
    pub parent_beacon_block_root: H256,
    /// The block bloom of this block, represented as the consecutive
    /// 32-byte chunks of a block's final bloom filter string.
    pub block_bloom: [U256; 8],
}

impl BlockMetadata {
    pub fn from_public_inputs<F: RichField>(pis: &[F]) -> Self {
        assert!(pis.len() == BlockMetadataTarget::SIZE);

        let block_beneficiary = get_h160(&pis[0..5]);
        let block_timestamp = pis[5].to_canonical_u64().into();
        let block_number = pis[6].to_canonical_u64().into();
        let block_difficulty = pis[7].to_canonical_u64().into();
        let block_random = get_h256(&pis[8..16]);
        let block_gaslimit = pis[16].to_canonical_u64().into();
        let block_chain_id = pis[17].to_canonical_u64().into();
        let block_base_fee =
            (pis[18].to_canonical_u64() + (pis[19].to_canonical_u64() << 32)).into();
        let block_gas_used = pis[20].to_canonical_u64().into();
        let block_blob_gas_used =
            (pis[21].to_canonical_u64() + (pis[22].to_canonical_u64() << 32)).into();
        let block_excess_blob_gas =
            (pis[23].to_canonical_u64() + (pis[24].to_canonical_u64() << 32)).into();
        let parent_beacon_block_root = get_h256(&pis[25..33]);
        let block_bloom =
            core::array::from_fn(|i| h2u(get_h256(&pis[33 + 8 * i..33 + 8 * (i + 1)])));

        Self {
            block_beneficiary,
            block_timestamp,
            block_number,
            block_difficulty,
            block_random,
            block_gaslimit,
            block_chain_id,
            block_base_fee,
            block_gas_used,
            block_blob_gas_used,
            block_excess_blob_gas,
            parent_beacon_block_root,
            block_bloom,
        }
    }
}

/// Additional block data that are specific to the local transaction being
/// proven, unlike `BlockMetadata`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ExtraBlockData {
    /// The state trie digest of the checkpoint block.
    pub checkpoint_state_trie_root: H256,
    /// The transaction count prior execution of the local state transition,
    /// starting at 0 for the initial transaction of a block.
    pub txn_number_before: U256,
    /// The transaction count after execution of the local state transition.
    pub txn_number_after: U256,
    /// The accumulated gas used prior execution of the local state transition,
    /// starting at 0 for the initial transaction of a block.
    pub gas_used_before: U256,
    /// The accumulated gas used after execution of the local state transition.
    /// It should match the `block_gas_used` value after execution of the
    /// last transaction in a block.
    pub gas_used_after: U256,
}

impl ExtraBlockData {
    pub fn from_public_inputs<F: RichField>(pis: &[F]) -> Self {
        assert!(pis.len() == ExtraBlockDataTarget::SIZE);

        let checkpoint_state_trie_root = get_h256(&pis[0..8]);
        let txn_number_before = pis[8].to_canonical_u64().into();
        let txn_number_after = pis[9].to_canonical_u64().into();
        let gas_used_before = pis[10].to_canonical_u64().into();
        let gas_used_after = pis[11].to_canonical_u64().into();

        Self {
            checkpoint_state_trie_root,
            txn_number_before,
            txn_number_after,
            gas_used_before,
            gas_used_after,
        }
    }
}

/// Registers data used to preinitialize the registers and check the final
/// registers of the current proof.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct RegistersData {
    /// Program counter.
    pub program_counter: U256,
    /// Indicates whether we are in kernel mode.
    pub is_kernel: U256,
    /// Stack length.
    pub stack_len: U256,
    /// Top of the stack.
    pub stack_top: U256,
    /// Context.
    pub context: U256,
    /// Gas used so far.
    pub gas_used: U256,
}

impl RegistersData {
    pub fn from_public_inputs<F: RichField>(pis: &[F]) -> Self {
        assert!(pis.len() == RegistersDataTarget::SIZE);

        let program_counter = pis[0].to_canonical_u64().into();
        let is_kernel = pis[1].to_canonical_u64().into();
        let stack_len = pis[2].to_canonical_u64().into();
        let stack_top = get_u256(&pis[3..11].try_into().unwrap());
        let context = pis[11].to_canonical_u64().into();
        let gas_used = pis[12].to_canonical_u64().into();

        Self {
            program_counter,
            is_kernel,
            stack_len,
            stack_top,
            context,
            gas_used,
        }
    }
}

impl From<RegistersState> for RegistersData {
    fn from(registers: RegistersState) -> Self {
        RegistersData {
            program_counter: registers.program_counter.into(),
            is_kernel: (registers.is_kernel as u64).into(),
            stack_len: registers.stack_len.into(),
            stack_top: registers.stack_top,
            context: registers.context.into(),
            gas_used: registers.gas_used.into(),
        }
    }
}

/// Structure for a Merkle cap. It is used for `MemBefore` and `MemAfter`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct MemCap {
    /// STARK cap.
    pub mem_cap: Vec<[U256; NUM_HASH_OUT_ELTS]>,
}

impl MemCap {
    pub fn from_public_inputs<F: RichField>(pis: &[F]) -> Self {
        let mem_cap = (0..DEFAULT_CAP_LEN)
            .map(|i| {
                core::array::from_fn(|j| {
                    U256::from(pis[pis.len() - 4 * (DEFAULT_CAP_LEN - i) + j].to_canonical_u64())
                })
            })
            .collect();

        Self { mem_cap }
    }
}

/// Memory values which are public.
/// Note: All the larger integers are encoded with 32-bit limbs in little-endian
/// order.
#[derive(Eq, PartialEq, Debug)]
pub struct PublicValuesTarget {
    /// Trie hashes before the execution of the local state transition.
    pub trie_roots_before: TrieRootsTarget,
    /// Trie hashes after the execution of the local state transition.
    pub trie_roots_after: TrieRootsTarget,
    /// Address to store the base fee to be burnt.
    pub burn_addr: BurnAddrTarget,
    /// Block metadata: it remains unchanged within a block.
    pub block_metadata: BlockMetadataTarget,
    /// 256 previous block hashes and current block's hash.
    pub block_hashes: BlockHashesTarget,
    /// Extra block data that is specific to the current proof.
    pub extra_block_data: ExtraBlockDataTarget,
    /// Registers to initialize the current proof.
    pub registers_before: RegistersDataTarget,
    /// Registers at the end of the current proof.
    pub registers_after: RegistersDataTarget,
    /// Memory before.
    pub mem_before: MemCapTarget,
    /// Memory after.
    pub mem_after: MemCapTarget,
}

impl PublicValuesTarget {
    pub(crate) const SIZE: usize = TrieRootsTarget::SIZE * 2
        + BlockMetadataTarget::SIZE
        + BlockHashesTarget::SIZE
        + ExtraBlockDataTarget::SIZE
        + DEFAULT_CAP_HEIGHT * NUM_HASH_OUT_ELTS * 2;
    /// Serializes public value targets.
    pub(crate) fn to_buffer(&self, buffer: &mut Vec<u8>) -> IoResult<()> {
        let TrieRootsTarget {
            state_root: state_root_before,
            transactions_root: transactions_root_before,
            receipts_root: receipts_root_before,
        } = self.trie_roots_before;

        buffer.write_target_array(&state_root_before)?;
        buffer.write_target_array(&transactions_root_before)?;
        buffer.write_target_array(&receipts_root_before)?;

        let TrieRootsTarget {
            state_root: state_root_after,
            transactions_root: transactions_root_after,
            receipts_root: receipts_root_after,
        } = self.trie_roots_after;

        buffer.write_target_array(&state_root_after)?;
        buffer.write_target_array(&transactions_root_after)?;
        buffer.write_target_array(&receipts_root_after)?;

        let BlockMetadataTarget {
            block_beneficiary,
            block_timestamp,
            block_number,
            block_difficulty,
            block_random,
            block_gaslimit,
            block_chain_id,
            block_base_fee,
            block_gas_used,
            block_blob_gas_used,
            block_excess_blob_gas,
            parent_beacon_block_root,
            block_bloom,
        } = self.block_metadata;

        buffer.write_target_array(&block_beneficiary)?;
        buffer.write_target(block_timestamp)?;
        buffer.write_target(block_number)?;
        buffer.write_target(block_difficulty)?;
        buffer.write_target_array(&block_random)?;
        buffer.write_target(block_gaslimit)?;
        buffer.write_target(block_chain_id)?;
        buffer.write_target_array(&block_base_fee)?;
        buffer.write_target(block_gas_used)?;
        buffer.write_target_array(&block_blob_gas_used)?;
        buffer.write_target_array(&block_excess_blob_gas)?;
        buffer.write_target_array(&parent_beacon_block_root)?;
        buffer.write_target_array(&block_bloom)?;

        let BlockHashesTarget {
            prev_hashes,
            cur_hash,
        } = self.block_hashes;
        buffer.write_target_array(&prev_hashes)?;
        buffer.write_target_array(&cur_hash)?;

        let ExtraBlockDataTarget {
            checkpoint_state_trie_root,
            txn_number_before,
            txn_number_after,
            gas_used_before,
            gas_used_after,
        } = self.extra_block_data;
        buffer.write_target_array(&checkpoint_state_trie_root)?;
        buffer.write_target(txn_number_before)?;
        buffer.write_target(txn_number_after)?;
        buffer.write_target(gas_used_before)?;
        buffer.write_target(gas_used_after)?;
        let RegistersDataTarget {
            program_counter: program_counter_before,
            is_kernel: is_kernel_before,
            stack_len: stack_len_before,
            stack_top: stack_top_before,
            context: context_before,
            gas_used: gas_used_before,
        } = self.registers_before;
        buffer.write_target(program_counter_before)?;
        buffer.write_target(is_kernel_before)?;
        buffer.write_target(stack_len_before)?;
        buffer.write_target_array(&stack_top_before)?;
        buffer.write_target(context_before)?;
        buffer.write_target(gas_used_before)?;
        let RegistersDataTarget {
            program_counter: program_counter_after,
            is_kernel: is_kernel_after,
            stack_len: stack_len_after,
            stack_top: stack_top_after,
            context: context_after,
            gas_used: gas_used_after,
        } = self.registers_after;
        buffer.write_target(program_counter_after)?;
        buffer.write_target(is_kernel_after)?;
        buffer.write_target(stack_len_after)?;
        buffer.write_target_array(&stack_top_after)?;
        buffer.write_target(context_after)?;
        buffer.write_target(gas_used_after)?;

        buffer.write_target_merkle_cap(&self.mem_before.mem_cap)?;
        buffer.write_target_merkle_cap(&self.mem_after.mem_cap)?;

        Ok(())
    }

    /// Deserializes public value targets.
    pub(crate) fn from_buffer(buffer: &mut Buffer) -> IoResult<Self> {
        let trie_roots_before = TrieRootsTarget {
            state_root: buffer.read_target_array()?,
            transactions_root: buffer.read_target_array()?,
            receipts_root: buffer.read_target_array()?,
        };

        let trie_roots_after = TrieRootsTarget {
            state_root: buffer.read_target_array()?,
            transactions_root: buffer.read_target_array()?,
            receipts_root: buffer.read_target_array()?,
        };

        let burn_addr = match cfg!(feature = "cdk_erigon") {
            true => BurnAddrTarget::BurnAddr(buffer.read_target_array()?),
            false => BurnAddrTarget::Burnt(),
        };

        let block_metadata = BlockMetadataTarget {
            block_beneficiary: buffer.read_target_array()?,
            block_timestamp: buffer.read_target()?,
            block_number: buffer.read_target()?,
            block_difficulty: buffer.read_target()?,
            block_random: buffer.read_target_array()?,
            block_gaslimit: buffer.read_target()?,
            block_chain_id: buffer.read_target()?,
            block_base_fee: buffer.read_target_array()?,
            block_gas_used: buffer.read_target()?,
            block_blob_gas_used: buffer.read_target_array()?,
            block_excess_blob_gas: buffer.read_target_array()?,
            parent_beacon_block_root: buffer.read_target_array()?,
            block_bloom: buffer.read_target_array()?,
        };

        let block_hashes = BlockHashesTarget {
            prev_hashes: buffer.read_target_array()?,
            cur_hash: buffer.read_target_array()?,
        };

        let extra_block_data = ExtraBlockDataTarget {
            checkpoint_state_trie_root: buffer.read_target_array()?,
            txn_number_before: buffer.read_target()?,
            txn_number_after: buffer.read_target()?,
            gas_used_before: buffer.read_target()?,
            gas_used_after: buffer.read_target()?,
        };

        let registers_before = RegistersDataTarget {
            program_counter: buffer.read_target()?,
            is_kernel: buffer.read_target()?,
            stack_len: buffer.read_target()?,
            stack_top: buffer.read_target_array()?,
            context: buffer.read_target()?,
            gas_used: buffer.read_target()?,
        };
        let registers_after = RegistersDataTarget {
            program_counter: buffer.read_target()?,
            is_kernel: buffer.read_target()?,
            stack_len: buffer.read_target()?,
            stack_top: buffer.read_target_array()?,
            context: buffer.read_target()?,
            gas_used: buffer.read_target()?,
        };

        let mem_before = MemCapTarget {
            mem_cap: buffer.read_target_merkle_cap()?,
        };
        let mem_after = MemCapTarget {
            mem_cap: buffer.read_target_merkle_cap()?,
        };

        Ok(Self {
            trie_roots_before,
            trie_roots_after,
            burn_addr,
            block_metadata,
            block_hashes,
            extra_block_data,
            registers_before,
            registers_after,
            mem_before,
            mem_after,
        })
    }

    /// Extracts public value `Target`s from the given public input `Target`s.
    /// Public values are always the first public inputs added to the circuit,
    /// so we can start extracting at index 0.
    pub(crate) fn from_public_inputs(pis: &[Target]) -> Self {
        assert!(pis.len() >= Self::SIZE);

        let mut offset = 0;
        let trie_roots_before =
            TrieRootsTarget::from_public_inputs(&pis[offset..offset + TrieRootsTarget::SIZE]);
        offset += TrieRootsTarget::SIZE;
        let trie_roots_after =
            TrieRootsTarget::from_public_inputs(&pis[offset..offset + TrieRootsTarget::SIZE]);
        offset += TrieRootsTarget::SIZE;
        let burn_addr =
            BurnAddrTarget::from_public_inputs(&pis[offset..offset + BurnAddrTarget::get_size()]);
        offset += BurnAddrTarget::get_size();
        let block_metadata = BlockMetadataTarget::from_public_inputs(
            &pis[offset..offset + BlockMetadataTarget::SIZE],
        );
        offset += BlockMetadataTarget::SIZE;
        let block_hashes =
            BlockHashesTarget::from_public_inputs(&pis[offset..offset + BlockHashesTarget::SIZE]);
        offset += BlockHashesTarget::SIZE;
        let extra_block_data = ExtraBlockDataTarget::from_public_inputs(
            &pis[offset..offset + ExtraBlockDataTarget::SIZE],
        );
        offset += ExtraBlockDataTarget::SIZE;
        let registers_before = RegistersDataTarget::from_public_inputs(
            &pis[offset..offset + RegistersDataTarget::SIZE],
        );
        offset += RegistersDataTarget::SIZE;
        let registers_after = RegistersDataTarget::from_public_inputs(
            &pis[offset..offset + RegistersDataTarget::SIZE],
        );
        offset += RegistersDataTarget::SIZE;
        let mem_before =
            MemCapTarget::from_public_inputs(&pis[offset..offset + MemCapTarget::SIZE]);
        offset += MemCapTarget::SIZE;
        let mem_after = MemCapTarget::from_public_inputs(&pis[offset..offset + MemCapTarget::SIZE]);

        Self {
            trie_roots_before,
            trie_roots_after,
            burn_addr,
            block_metadata,
            block_hashes,
            extra_block_data,
            registers_before,
            registers_after,
            mem_before,
            mem_after,
        }
    }

    /// Returns the public values in `pv0` or `pv1` depending on `condition`.
    pub(crate) fn select<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        pv0: Self,
        pv1: Self,
    ) -> Self {
        Self {
            trie_roots_before: TrieRootsTarget::select(
                builder,
                condition,
                pv0.trie_roots_before,
                pv1.trie_roots_before,
            ),
            trie_roots_after: TrieRootsTarget::select(
                builder,
                condition,
                pv0.trie_roots_after,
                pv1.trie_roots_after,
            ),
            burn_addr: BurnAddrTarget::select(builder, condition, pv0.burn_addr, pv1.burn_addr),
            block_metadata: BlockMetadataTarget::select(
                builder,
                condition,
                pv0.block_metadata,
                pv1.block_metadata,
            ),
            block_hashes: BlockHashesTarget::select(
                builder,
                condition,
                pv0.block_hashes,
                pv1.block_hashes,
            ),
            extra_block_data: ExtraBlockDataTarget::select(
                builder,
                condition,
                pv0.extra_block_data,
                pv1.extra_block_data,
            ),
            registers_before: RegistersDataTarget::select(
                builder,
                condition,
                pv0.registers_before,
                pv1.registers_before,
            ),
            registers_after: RegistersDataTarget::select(
                builder,
                condition,
                pv0.registers_after,
                pv1.registers_after,
            ),
            mem_before: MemCapTarget::select(builder, condition, pv0.mem_before, pv1.mem_before),

            mem_after: MemCapTarget::select(builder, condition, pv0.mem_after, pv1.mem_after),
        }
    }
}

/// Circuit version of `TrieRoots`.
/// `Target`s for trie hashes. Since a `Target` holds a 32-bit limb, each hash
/// requires 8 `Target`s.
#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub struct TrieRootsTarget {
    /// Targets for the state trie hash.
    pub(crate) state_root: [Target; TARGET_HASH_SIZE],
    /// Targets for the transactions trie hash.
    pub(crate) transactions_root: [Target; TARGET_HASH_SIZE],
    /// Targets for the receipts trie hash.
    pub(crate) receipts_root: [Target; TARGET_HASH_SIZE],
}

/// Number of `Target`s required for hashes.
pub(crate) const TARGET_HASH_SIZE: usize = 8;

impl TrieRootsTarget {
    pub(crate) const SIZE: usize = TARGET_HASH_SIZE * 3;

    /// Extracts trie hash `Target`s for all tries from the provided public
    /// input `Target`s. The provided `pis` should start with the trie
    /// hashes.
    pub(crate) fn from_public_inputs(pis: &[Target]) -> Self {
        let state_root = pis[0..TARGET_HASH_SIZE].try_into().unwrap();
        let transactions_root = pis[TARGET_HASH_SIZE..2 * TARGET_HASH_SIZE]
            .try_into()
            .unwrap();
        let receipts_root = pis[2 * TARGET_HASH_SIZE..3 * TARGET_HASH_SIZE]
            .try_into()
            .unwrap();

        Self {
            state_root,
            transactions_root,
            receipts_root,
        }
    }

    /// If `condition`, returns the trie hashes in `tr0`,
    /// otherwise returns the trie hashes in `tr1`.
    pub(crate) fn select<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        tr0: Self,
        tr1: Self,
    ) -> Self {
        Self {
            state_root: core::array::from_fn(|i| {
                builder.select(condition, tr0.state_root[i], tr1.state_root[i])
            }),
            transactions_root: core::array::from_fn(|i| {
                builder.select(
                    condition,
                    tr0.transactions_root[i],
                    tr1.transactions_root[i],
                )
            }),
            receipts_root: core::array::from_fn(|i| {
                builder.select(condition, tr0.receipts_root[i], tr1.receipts_root[i])
            }),
        }
    }

    /// Connects the trie hashes in `tr0` and in `tr1`.
    pub(crate) fn connect<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        tr0: Self,
        tr1: Self,
    ) {
        for i in 0..8 {
            builder.connect(tr0.state_root[i], tr1.state_root[i]);
            builder.connect(tr0.transactions_root[i], tr1.transactions_root[i]);
            builder.connect(tr0.receipts_root[i], tr1.receipts_root[i]);
        }
    }

    /// If `condition`, asserts that `tr0 == tr1`.
    pub(crate) fn conditional_assert_eq<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        tr0: Self,
        tr1: Self,
    ) {
        for i in 0..8 {
            builder.conditional_assert_eq(condition.target, tr0.state_root[i], tr1.state_root[i]);
            builder.conditional_assert_eq(
                condition.target,
                tr0.transactions_root[i],
                tr1.transactions_root[i],
            );
            builder.conditional_assert_eq(
                condition.target,
                tr0.receipts_root[i],
                tr1.receipts_root[i],
            );
        }
    }
}

/// Circuit version of `BurnAddr`.
/// Address used to store the base fee to be burnt.
#[derive(Eq, PartialEq, Debug, Clone)]
pub enum BurnAddrTarget {
    BurnAddr([Target; 8]),
    Burnt(),
}

impl BurnAddrTarget {
    pub const fn get_size() -> usize {
        match cfg!(feature = "cdk_erigon") {
            true => 8,
            false => 0,
        }
    }

    /// Extracts the burn address from the provided public input
    /// `Target`s. The provided `pis` should start with the burn address.
    pub(crate) fn from_public_inputs(pis: &[Target]) -> Self {
        match cfg!(feature = "cdk_erigon") {
            true => BurnAddrTarget::BurnAddr(pis[0..8].try_into().unwrap()),
            false => BurnAddrTarget::Burnt(),
        }
    }

    /// If `condition`, returns the burn address in `ba0`,
    /// otherwise returns the burn address in `ba1`.
    /// This is a no-op if `cdk_erigon` feature is not activated.  
    ///  
    /// This will panic if the `cdk_erigon` is activated and not both
    /// `BurnAddrTarget`s are `BurnAddr` variants.
    pub(crate) fn select<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        ba0: Self,
        ba1: Self,
    ) -> Self {
        match cfg!(feature = "cdk_erigon") {
            // If the `cdk_erigon` feature is activated, both `ba0` and `ba1` should be of type
            // `BurnAddr`.
            true => match (ba0, ba1) {
                (BurnAddrTarget::BurnAddr(a0), BurnAddrTarget::BurnAddr(a1)) => {
                    BurnAddrTarget::BurnAddr(core::array::from_fn(|i| {
                        builder.select(condition, a0[i], a1[i])
                    }))
                }
                _ => panic!("We should have already set an address (or U256::MAX) before."),
            },
            false => BurnAddrTarget::Burnt(),
        }
    }

    /// Connects the burn address in `ba0` to the burn address in `ba1`.
    /// This is a no-op if `cdk_erigon` feature is not activated.  
    ///  
    /// This will panic if the `cdk_erigon` is activated and not both
    /// `BurnAddrTarget`s are `BurnAddr` variants.
    pub(crate) fn connect<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        ba0: Self,
        ba1: Self,
    ) {
        // There only are targets to connect if there is a burn address, i.e. when the
        // `cdk_erigon` feature is active.
        if cfg!(feature = "cdk_erigon") == true {
            // If the `cdk_erigon` feature is activated, both `ba0` and `ba1` should be of
            // type `BurnAddr`.
            match (ba0, ba1) {
                (BurnAddrTarget::BurnAddr(a0), BurnAddrTarget::BurnAddr(a1)) => {
                    for i in 0..BurnAddrTarget::get_size() {
                        builder.connect(a0[i], a1[i]);
                    }
                }
                _ => panic!("We should have already set an address (or U256::MAX) before."),
            }
        }
    }

    /// If `condition`, asserts that `ba0 == ba1`.
    /// This is a no-op if `cdk_erigon` feature is not activated.  
    ///  
    /// This will panic if the `cdk_erigon` is activated and not both
    /// `BurnAddrTarget` are `BurnAddr` variants.
    pub(crate) fn conditional_assert_eq<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        ba0: Self,
        ba1: Self,
    ) {
        if cfg!(feature = "cdk_erigon") {
            match (ba0, ba1) {
                (
                    BurnAddrTarget::BurnAddr(addr_targets_0),
                    BurnAddrTarget::BurnAddr(addr_targets_1),
                ) => {
                    for i in 0..BurnAddrTarget::get_size() {
                        builder.conditional_assert_eq(
                            condition.target,
                            addr_targets_0[i],
                            addr_targets_1[i],
                        )
                    }
                }
                _ => panic!("There should be an address set in cdk_erigon."),
            }
        }
    }
}

/// Circuit version of `BlockMetadata`.
/// Metadata contained in a block header. Those are identical between
/// all state transition proofs within the same block.
#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub struct BlockMetadataTarget {
    /// `Target`s for the address of this block's producer.
    pub(crate) block_beneficiary: [Target; 5],
    /// `Target` for the timestamp of this block.
    pub(crate) block_timestamp: Target,
    /// `Target` for the index of this block.
    pub(crate) block_number: Target,
    /// `Target` for the difficulty (before PoS transition) of this block.
    pub(crate) block_difficulty: Target,
    /// `Target`s for the `mix_hash` value of this block.
    pub(crate) block_random: [Target; 8],
    /// `Target` for the gas limit of this block.
    pub(crate) block_gaslimit: Target,
    /// `Target` for the chain id of this block.
    pub(crate) block_chain_id: Target,
    /// `Target`s for the base fee of this block.
    pub(crate) block_base_fee: [Target; 2],
    /// `Target` for the gas used of this block.
    pub(crate) block_gas_used: Target,
    /// `Target`s for the total blob gas used of this block.
    pub(crate) block_blob_gas_used: [Target; 2],
    /// `Target`s for the excess blob gas of this block.
    pub(crate) block_excess_blob_gas: [Target; 2],
    /// `Target`s for the block bloom of this block.
    /// `Target`s for the parent beacon block root.
    pub(crate) parent_beacon_block_root: [Target; 8],
    pub(crate) block_bloom: [Target; 64],
}

impl BlockMetadataTarget {
    /// Number of `Target`s required for the block metadata.
    pub(crate) const SIZE: usize = 97;

    /// Extracts block metadata `Target`s from the provided public input
    /// `Target`s. The provided `pis` should start with the block metadata.
    pub(crate) fn from_public_inputs(pis: &[Target]) -> Self {
        let block_beneficiary = pis[0..5].try_into().unwrap();
        let block_timestamp = pis[5];
        let block_number = pis[6];
        let block_difficulty = pis[7];
        let block_random = pis[8..16].try_into().unwrap();
        let block_gaslimit = pis[16];
        let block_chain_id = pis[17];
        let block_base_fee = pis[18..20].try_into().unwrap();
        let block_gas_used = pis[20];
        let block_blob_gas_used = pis[21..23].try_into().unwrap();
        let block_excess_blob_gas = pis[23..25].try_into().unwrap();
        let parent_beacon_block_root = pis[25..33].try_into().unwrap();
        let block_bloom = pis[33..97].try_into().unwrap();

        Self {
            block_beneficiary,
            block_timestamp,
            block_number,
            block_difficulty,
            block_random,
            block_gaslimit,
            block_chain_id,
            block_base_fee,
            block_gas_used,
            block_blob_gas_used,
            block_excess_blob_gas,
            parent_beacon_block_root,
            block_bloom,
        }
    }

    /// If `condition`, returns the block metadata in `bm0`,
    /// otherwise returns the block metadata in `bm1`.
    pub(crate) fn select<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        bm0: Self,
        bm1: Self,
    ) -> Self {
        Self {
            block_beneficiary: core::array::from_fn(|i| {
                builder.select(
                    condition,
                    bm0.block_beneficiary[i],
                    bm1.block_beneficiary[i],
                )
            }),
            block_timestamp: builder.select(condition, bm0.block_timestamp, bm1.block_timestamp),
            block_number: builder.select(condition, bm0.block_number, bm1.block_number),
            block_difficulty: builder.select(condition, bm0.block_difficulty, bm1.block_difficulty),
            block_random: core::array::from_fn(|i| {
                builder.select(condition, bm0.block_random[i], bm1.block_random[i])
            }),
            block_gaslimit: builder.select(condition, bm0.block_gaslimit, bm1.block_gaslimit),
            block_chain_id: builder.select(condition, bm0.block_chain_id, bm1.block_chain_id),
            block_base_fee: core::array::from_fn(|i| {
                builder.select(condition, bm0.block_base_fee[i], bm1.block_base_fee[i])
            }),
            block_gas_used: builder.select(condition, bm0.block_gas_used, bm1.block_gas_used),
            block_blob_gas_used: core::array::from_fn(|i| {
                builder.select(
                    condition,
                    bm0.block_blob_gas_used[i],
                    bm1.block_blob_gas_used[i],
                )
            }),
            block_excess_blob_gas: core::array::from_fn(|i| {
                builder.select(
                    condition,
                    bm0.block_excess_blob_gas[i],
                    bm1.block_excess_blob_gas[i],
                )
            }),
            parent_beacon_block_root: core::array::from_fn(|i| {
                builder.select(
                    condition,
                    bm0.parent_beacon_block_root[i],
                    bm1.parent_beacon_block_root[i],
                )
            }),
            block_bloom: core::array::from_fn(|i| {
                builder.select(condition, bm0.block_bloom[i], bm1.block_bloom[i])
            }),
        }
    }

    /// Connects the block metadata in `bm0` to the block metadata in `bm1`.
    pub(crate) fn connect<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        bm0: Self,
        bm1: Self,
    ) {
        for i in 0..5 {
            builder.connect(bm0.block_beneficiary[i], bm1.block_beneficiary[i]);
        }
        builder.connect(bm0.block_timestamp, bm1.block_timestamp);
        builder.connect(bm0.block_number, bm1.block_number);
        builder.connect(bm0.block_difficulty, bm1.block_difficulty);
        for i in 0..8 {
            builder.connect(bm0.block_random[i], bm1.block_random[i]);
        }
        builder.connect(bm0.block_gaslimit, bm1.block_gaslimit);
        builder.connect(bm0.block_chain_id, bm1.block_chain_id);
        for i in 0..2 {
            builder.connect(bm0.block_base_fee[i], bm1.block_base_fee[i])
        }
        builder.connect(bm0.block_gas_used, bm1.block_gas_used);
        for i in 0..2 {
            builder.connect(bm0.block_blob_gas_used[i], bm1.block_blob_gas_used[i])
        }
        for i in 0..2 {
            builder.connect(bm0.block_excess_blob_gas[i], bm1.block_excess_blob_gas[i])
        }
        for i in 0..8 {
            builder.connect(
                bm0.parent_beacon_block_root[i],
                bm1.parent_beacon_block_root[i],
            )
        }
        for i in 0..64 {
            builder.connect(bm0.block_bloom[i], bm1.block_bloom[i])
        }
    }

    /// If `condition`, asserts that `bm0 == bm1`.
    pub(crate) fn conditional_assert_eq<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        bm0: Self,
        bm1: Self,
    ) {
        for i in 0..5 {
            builder.conditional_assert_eq(
                condition.target,
                bm0.block_beneficiary[i],
                bm1.block_beneficiary[i],
            );
        }
        builder.conditional_assert_eq(condition.target, bm0.block_timestamp, bm1.block_timestamp);
        builder.conditional_assert_eq(condition.target, bm0.block_number, bm1.block_number);
        builder.conditional_assert_eq(condition.target, bm0.block_difficulty, bm1.block_difficulty);
        for i in 0..8 {
            builder.conditional_assert_eq(
                condition.target,
                bm0.block_random[i],
                bm1.block_random[i],
            );
        }
        builder.conditional_assert_eq(condition.target, bm0.block_gaslimit, bm1.block_gaslimit);
        builder.conditional_assert_eq(condition.target, bm0.block_chain_id, bm1.block_chain_id);
        for i in 0..2 {
            builder.conditional_assert_eq(
                condition.target,
                bm0.block_base_fee[i],
                bm1.block_base_fee[i],
            )
        }
        builder.conditional_assert_eq(condition.target, bm0.block_gas_used, bm1.block_gas_used);
        for i in 0..64 {
            builder.conditional_assert_eq(condition.target, bm0.block_bloom[i], bm1.block_bloom[i])
        }
    }
}

/// Circuit version of `BlockHashes`.
/// `Target`s for the user-provided previous 256 block hashes and current block
/// hash. Each block hash requires 8 `Target`s.
/// The proofs across consecutive blocks ensure that these values
/// are consistent (i.e. shifted by eight `Target`s to the left).
///
/// When the block number is less than 256, dummy values, i.e.
/// `H256::default()`, should be used for the additional block hashes.
#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub struct BlockHashesTarget {
    /// `Target`s for the previous 256 hashes to the current block. The leftmost
    /// hash, i.e. `prev_hashes[0..8]`, is the oldest, and the rightmost,
    /// i.e. `prev_hashes[255 * 7..255 * 8]` is the hash of the parent block.
    pub(crate) prev_hashes: [Target; 2048],
    // `Target` for the hash of the current block.
    pub(crate) cur_hash: [Target; 8],
}

impl BlockHashesTarget {
    /// Number of `Target`s required for previous and current block hashes.
    pub(crate) const SIZE: usize = 2056;

    /// Extracts the previous and current block hash `Target`s from the public
    /// input `Target`s. The provided `pis` should start with the block
    /// hashes.
    pub(crate) fn from_public_inputs(pis: &[Target]) -> Self {
        Self {
            prev_hashes: pis[0..2048].try_into().unwrap(),
            cur_hash: pis[2048..2056].try_into().unwrap(),
        }
    }

    /// If `condition`, returns the block hashes in `bm0`,
    /// otherwise returns the block hashes in `bm1`.
    pub(crate) fn select<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        bm0: Self,
        bm1: Self,
    ) -> Self {
        Self {
            prev_hashes: core::array::from_fn(|i| {
                builder.select(condition, bm0.prev_hashes[i], bm1.prev_hashes[i])
            }),
            cur_hash: core::array::from_fn(|i| {
                builder.select(condition, bm0.cur_hash[i], bm1.cur_hash[i])
            }),
        }
    }

    /// Connects the block hashes in `bm0` to the block hashes in `bm1`.
    pub(crate) fn connect<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        bm0: Self,
        bm1: Self,
    ) {
        for i in 0..2048 {
            builder.connect(bm0.prev_hashes[i], bm1.prev_hashes[i]);
        }
        for i in 0..8 {
            builder.connect(bm0.cur_hash[i], bm1.cur_hash[i]);
        }
    }

    /// If `condition`, asserts that `bm0 == bm1`.
    pub(crate) fn conditional_assert_eq<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        bm0: Self,
        bm1: Self,
    ) {
        for i in 0..2048 {
            builder.conditional_assert_eq(condition.target, bm0.prev_hashes[i], bm1.prev_hashes[i]);
        }
        for i in 0..8 {
            builder.conditional_assert_eq(condition.target, bm0.cur_hash[i], bm1.cur_hash[i]);
        }
    }
}

/// Circuit version of `ExtraBlockData`.
/// Additional block data that are specific to the local transaction being
/// proven, unlike `BlockMetadata`.
#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub struct ExtraBlockDataTarget {
    /// `Target`s for the state trie digest of the checkpoint block.
    pub checkpoint_state_trie_root: [Target; 8],
    /// `Target` for the transaction count prior execution of the local state
    /// transition, starting at 0 for the initial trnasaction of a block.
    pub txn_number_before: Target,
    /// `Target` for the transaction count after execution of the local state
    /// transition.
    pub txn_number_after: Target,
    /// `Target` for the accumulated gas used prior execution of the local state
    /// transition, starting at 0 for the initial transaction of a block.
    pub gas_used_before: Target,
    /// `Target` for the accumulated gas used after execution of the local state
    /// transition. It should match the `block_gas_used` value after
    /// execution of the last transaction in a block.
    pub gas_used_after: Target,
}

impl ExtraBlockDataTarget {
    /// Number of `Target`s required for the extra block data.
    pub(crate) const SIZE: usize = 12;

    /// Extracts the extra block data `Target`s from the public input `Target`s.
    /// The provided `pis` should start with the extra vblock data.
    pub(crate) fn from_public_inputs(pis: &[Target]) -> Self {
        let checkpoint_state_trie_root = pis[0..8].try_into().unwrap();
        let txn_number_before = pis[8];
        let txn_number_after = pis[9];
        let gas_used_before = pis[10];
        let gas_used_after = pis[11];

        Self {
            checkpoint_state_trie_root,
            txn_number_before,
            txn_number_after,
            gas_used_before,
            gas_used_after,
        }
    }

    /// If `condition`, returns the extra block data in `ed0`,
    /// otherwise returns the extra block data in `ed1`.
    pub(crate) fn select<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        ed0: Self,
        ed1: Self,
    ) -> Self {
        Self {
            checkpoint_state_trie_root: core::array::from_fn(|i| {
                builder.select(
                    condition,
                    ed0.checkpoint_state_trie_root[i],
                    ed1.checkpoint_state_trie_root[i],
                )
            }),
            txn_number_before: builder.select(
                condition,
                ed0.txn_number_before,
                ed1.txn_number_before,
            ),
            txn_number_after: builder.select(condition, ed0.txn_number_after, ed1.txn_number_after),
            gas_used_before: builder.select(condition, ed0.gas_used_before, ed1.gas_used_before),
            gas_used_after: builder.select(condition, ed0.gas_used_after, ed1.gas_used_after),
        }
    }

    /// Connects the extra block data in `ed0` with the extra block data in
    /// `ed1`.
    pub(crate) fn connect<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        ed0: Self,
        ed1: Self,
    ) {
        for i in 0..8 {
            builder.connect(
                ed0.checkpoint_state_trie_root[i],
                ed1.checkpoint_state_trie_root[i],
            );
        }
        builder.connect(ed0.txn_number_before, ed1.txn_number_before);
        builder.connect(ed0.txn_number_after, ed1.txn_number_after);
        builder.connect(ed0.gas_used_before, ed1.gas_used_before);
        builder.connect(ed0.gas_used_after, ed1.gas_used_after);
    }

    /// If `condition`, asserts that `ed0 == ed1`.
    pub(crate) fn conditional_assert_eq<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        ed0: Self,
        ed1: Self,
    ) {
        for i in 0..8 {
            builder.conditional_assert_eq(
                condition.target,
                ed0.checkpoint_state_trie_root[i],
                ed1.checkpoint_state_trie_root[i],
            );
        }
        builder.conditional_assert_eq(
            condition.target,
            ed0.txn_number_before,
            ed1.txn_number_before,
        );
        builder.conditional_assert_eq(condition.target, ed0.txn_number_after, ed1.txn_number_after);
        builder.conditional_assert_eq(condition.target, ed0.gas_used_before, ed1.gas_used_before);
        builder.conditional_assert_eq(condition.target, ed0.gas_used_after, ed1.gas_used_after);
    }
}

/// Circuit version of `RegistersData`.
/// Registers data used to preinitialize the registers and check the final
/// registers of the current proof.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct RegistersDataTarget {
    /// Program counter.
    pub program_counter: Target,
    /// Indicates whether we are in kernel mode.
    pub is_kernel: Target,
    /// Stack length.
    pub stack_len: Target,
    /// Top of the stack.
    pub stack_top: [Target; 8],
    /// Context.
    pub context: Target,
    /// Gas used so far.
    pub gas_used: Target,
}

impl RegistersDataTarget {
    /// Number of `Target`s required for the extra block data.
    pub const SIZE: usize = 13;

    /// Extracts the extra block data `Target`s from the public input `Target`s.
    /// The provided `pis` should start with the extra vblock data.
    pub(crate) fn from_public_inputs(pis: &[Target]) -> Self {
        let program_counter = pis[0];
        let is_kernel = pis[1];
        let stack_len = pis[2];
        let stack_top = pis[3..11].try_into().unwrap();
        let context = pis[11];
        let gas_used = pis[12];

        Self {
            program_counter,
            is_kernel,
            stack_len,
            stack_top,
            context,
            gas_used,
        }
    }

    /// If `condition`, returns the extra block data in `ed0`,
    /// otherwise returns the extra block data in `ed1`.
    pub(crate) fn select<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        rd0: Self,
        rd1: Self,
    ) -> Self {
        Self {
            program_counter: builder.select(condition, rd0.program_counter, rd1.program_counter),
            is_kernel: builder.select(condition, rd0.is_kernel, rd1.is_kernel),
            stack_len: builder.select(condition, rd0.stack_len, rd1.stack_len),
            stack_top: core::array::from_fn(|i| {
                builder.select(condition, rd0.stack_top[i], rd1.stack_top[i])
            }),
            context: builder.select(condition, rd0.context, rd1.context),
            gas_used: builder.select(condition, rd0.gas_used, rd1.gas_used),
        }
    }

    /// Connects the extra block data in `ed0` with the extra block data in
    /// `ed1`.
    pub(crate) fn connect<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        rd0: Self,
        rd1: Self,
    ) {
        builder.connect(rd0.program_counter, rd1.program_counter);
        builder.connect(rd0.is_kernel, rd1.is_kernel);
        builder.connect(rd0.stack_len, rd1.stack_len);
        for i in 0..8 {
            builder.connect(rd0.stack_top[i], rd1.stack_top[i]);
        }
        builder.connect(rd0.context, rd1.context);
        builder.connect(rd0.gas_used, rd1.gas_used);
    }

    /// If `condition`, asserts that `rd0 == rd1`.
    pub(crate) fn conditional_assert_eq<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        rd0: Self,
        rd1: Self,
    ) {
        builder.conditional_assert_eq(condition.target, rd0.program_counter, rd1.program_counter);
        builder.conditional_assert_eq(condition.target, rd0.is_kernel, rd1.is_kernel);
        builder.conditional_assert_eq(condition.target, rd0.stack_len, rd1.stack_len);
        for i in 0..8 {
            builder.conditional_assert_eq(condition.target, rd0.stack_top[i], rd1.stack_top[i]);
        }
        builder.conditional_assert_eq(condition.target, rd0.context, rd1.context);
        builder.conditional_assert_eq(condition.target, rd0.gas_used, rd1.gas_used);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemCapTarget {
    /// Merkle cap.
    pub mem_cap: MerkleCapTarget,
}

impl MemCapTarget {
    pub(crate) const SIZE: usize = DEFAULT_CAP_LEN * NUM_HASH_OUT_ELTS;

    /// Extracts the exit kernel `Target`s from the public input `Target`s.
    /// The provided `pis` should start with the extra vblock data.
    pub(crate) fn from_public_inputs(pis: &[Target]) -> Self {
        let mem_values = &pis[0..Self::SIZE];
        let mem_cap = MerkleCapTarget(
            (0..DEFAULT_CAP_LEN)
                .map(|i| HashOutTarget {
                    elements: mem_values[i * NUM_HASH_OUT_ELTS..(i + 1) * NUM_HASH_OUT_ELTS]
                        .try_into()
                        .unwrap(),
                })
                .collect::<Vec<_>>(),
        );

        Self { mem_cap }
    }

    /// If `condition`, returns the exit kernel in `ek0`,
    /// otherwise returns the exit kernel in `ek1`.
    pub(crate) fn select<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        mc0: Self,
        mc1: Self,
    ) -> Self {
        Self {
            mem_cap: MerkleCapTarget(
                (0..mc0.mem_cap.0.len())
                    .map(|i| HashOutTarget {
                        elements: (0..NUM_HASH_OUT_ELTS)
                            .map(|j| {
                                builder.select(
                                    condition,
                                    mc0.mem_cap.0[i].elements[j],
                                    mc1.mem_cap.0[i].elements[j],
                                )
                            })
                            .collect::<Vec<_>>()
                            .try_into()
                            .unwrap(),
                    })
                    .collect::<Vec<_>>(),
            ),
        }
    }

    /// Connects the exit kernel in `ek0` with the exit kernel in `ek1`.
    pub(crate) fn connect<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        mc0: Self,
        mc1: Self,
    ) {
        for i in 0..mc0.mem_cap.0.len() {
            for j in 0..NUM_HASH_OUT_ELTS {
                builder.connect(mc0.mem_cap.0[i].elements[j], mc1.mem_cap.0[i].elements[j]);
            }
        }
    }

    /// If `condition`, asserts that `mc0 == mc1`.
    pub(crate) fn conditional_assert_eq<F: RichField + Extendable<D>, const D: usize>(
        builder: &mut CircuitBuilder<F, D>,
        condition: BoolTarget,
        mc0: Self,
        mc1: Self,
    ) {
        for i in 0..mc0.mem_cap.0.len() {
            for j in 0..NUM_HASH_OUT_ELTS {
                builder.conditional_assert_eq(
                    condition.target,
                    mc0.mem_cap.0[i].elements[j],
                    mc1.mem_cap.0[i].elements[j],
                );
            }
        }
    }
}
