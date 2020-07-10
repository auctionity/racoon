# Structure Racoon

# Overall structure

Racoon is composed of multiple mostly-independant blockchains each maintaining a state and processing transactions. These blockchains are structured as a tree, each blockchain having between 0 and multiple children and 1 parent (except the root of the tree having no parent). The root chain is called the **beacon chain** and all others are called **shard chains**.
The beacon chain is responsible of validators stakes and rewards, while shard chains are responsible of smart contracts and users (validator or not) transactions with these contracts.

## 
# Data structures
## `Validator`
- `sig_type` (4 bytes) : identifier of the signature scheme used by this validator. Allow to add new signature schemes in the future in a backward-compatible way.
- `pubkey_hash` (32 bytes) : hash of the public key of the validator. Hashing the key allow to have a fixed-size registered data regardless of the signature scheme. When verifying a validator signature, the public key is extracted from it and hashed to verify if it matches `pubkey_hash`. *secp256k1* (Bitcoin, Ethereum) signatures allow to recover the public key from the signature. Schemes without key recovery will have the complete public key appended to the signature to check against `pubkey_hash`.

The hash of this structure is called the **validator address**.


## `EpochValidator`
- `address` (32 bytes) : validator address (see `[Validator](#Validator)`).
- `power` (32 bytes) : percentage of the stake of this validator in the whole validator pool as a floating-point value between 0 and 1.


## `Epoch`
- `seed` (32 bytes) : value used as a seed in the deterministic pseudo-random number generator used to compute validators weights.
- `validators` (32 bytes) : Merkle root of a tree containing the list of `[EpochValidator](#EpochValidator)` (one for each validator) which will be used for this epoch calculations.


## `BeaconBlockHeader`
- `previous` (32 bytes) : hash the previous `BeaconBlockHeader` in the chain.
- `height` (8 bytes) : height of the block.
- `validator_index` (8 bytes) : index of this block validator in the current `[Epoch](#Epoch)``.validators`.
- `body` (32 bytes) : hash of `BeaconBlockBody`
- `finalized_headers` (32 bytes) : Merkle root of a tree containing the list of all neighbor last finalized `ShardBlockHeader` hashes on the beacon chain.
- `epoch_0` (32 bytes) : hash of `Epoch` for even epochs.
- `epoch_1` (32 bytes) : hash of `Epoch` for odd epochs.
- `world` (32 bytes) : hash of `[BeaconWorld](#BeaconWorld)`.

We will alias `epoch_0` as `epoch_current` in even epochs and `epoch_next` in odd epochs, and `epoch_1` as `epoch_current` in odd epochs and `epoch_next` in even epochs.
`epoch_current` will be used for current epoch consensus calculations, while `epoch_next` will be updated so it can be used in the next epoch.

Having 2 fields is necessary for all chains to use the same `Epoch` as the same time. If only one field were used, shards would lag behind the beacon until the beacon block with the new `Epoch` is finalized on the shard. By having 2 fields, `epoch_next` can be updated and reach all shards before the next epoch, allowing all chains to switch to the same `Epoch` at the same time.


## `BeaconWorld`
- `events_mmr` (32 bytes) : Root of a Merkle Mountain Range of all events emitted on the beacon chain since the genesis.
- `events_cmt` (32 bytes) : Root of a Comb Merkle Tree of all events emitted on the beacon chain since the genesis.
- `used_shards_events` (32 bytes) : Root of a Sparse Merkle Tree associating shard events to a boolean telling if it was used on the beacon shard before. Used to prevent replays of transfers of *kits* from shards to the beacon.
- `state` (32 bytes) : hash of `[BeaconState](#BeaconState)`.


## `BeaconState`
- `balances` (32 bytes) : Root of a Sparse Merkle Tree associating a validator address to its `[StakeBalances](#StakeBalances)`.
- `joining` (32 bytes) : Root of a Merkle Tree of stacking `[StakeChange](#StakeChange)`.
- `leaving` (32 bytes) : Root of a Merkle Tree of unstacking `[StakeChange](#StakeChange)`.


## `StakeBalances`
- `available` (32 bytes) : Amount of *kits* freely available for transactions.
- `staked` (32 bytes) : Amount of *kits* staked and not available for transactions.


## `StakeChange`
- `address` (32 bytes) : Address of the validator changing its stake.
- `amount` (32 bytes) : Amount of *kits* being staked/unstaked.
- `height` (32 bytes) : At which height this change was requested. Will determine at which height this change is performed on the validator `[StakeBalances](#StakeBalances)` and removed from the `joining` /`leaving` list.


