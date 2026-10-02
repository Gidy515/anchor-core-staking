# Anchor Core Staking

A Solana NFT staking program built with **Anchor** and **Metaplex Core**. NFT owners can stake collection assets by freezing them in place, then unstake after a configurable minimum period to receive newly minted reward tokens.

The NFT remains owned by the user throughout staking. The program uses Core's **Attributes** plugin to record staking state and its **FreezeDelegate** plugin to freeze and thaw the asset. Rewards are distributed as fungible tokens through a separate mint.

> This repository is a learning implementation. See [Current limitations](#current-limitations) before extending it or deploying it with real assets.

## Features

- Create a Metaplex Core collection with a program-derived update authority.
- Mint Core assets into the collection.
- Initialize collection-specific staking parameters and a reward mint.
- Record staking status and start time directly on each asset.
- Freeze assets while staked without transferring ownership into escrow.
- Enforce a minimum staking period measured in whole days.
- Thaw assets and mint rewards when unstaking succeeds.
- Create the owner's reward associated token account when needed.
- Preserve unrelated asset attributes during staking-state updates.

## How it works

1. **Create a collection.** The program creates a Core collection whose update authority is a PDA derived from that collection's address.
2. **Mint an asset.** A user creates a Core asset within the collection and becomes its owner.
3. **Initialize staking.** A caller pays to create the collection's configuration and a reward mint with six decimals.
4. **Stake.** The asset owner signs. The program writes staking attributes and installs a frozen FreezeDelegate plugin whose authority is the update authority.
5. **Wait.** The asset must remain staked for at least the configured number of completed days.
6. **Unstake.** The owner signs again. The program resets the staking attributes, thaws the asset, and mints the calculated reward into the owner's reward token account.

Staking does not require a separate stake account for each NFT. State is stored in the NFT's Attributes plugin. Core NFT assets themselves are not SPL token mints; the reward token uses a separate fungible-token mint.

## Technology stack

| Component | Repository version / role |
| --- | --- |
| Rust | On-chain program, edition 2021 |
| Anchor CLI | `0.31.1` in `Anchor.toml` |
| `anchor-lang` | `0.31.1`, with `init-if-needed` |
| `anchor-spl` | `0.31.1` |
| Rust `mpl-core` | `0.11.1`, with Anchor integration |
| TypeScript Anchor client | `^0.31.1` |
| TypeScript `mpl-core` | `^1.10.0` |
| Umi | `^1.5.1` |
| SPL Token client | `^0.4.14` |
| Mocha / ts-mocha | TypeScript integration tests |
| Surfpool | Local network, deployment runbooks, and test time travel |

The configured local program ID is:

```text
DgqZnG7wjLMrxkpR51bNyFGsbcPSaGrePmRW6GRegc7R
```

This ID appears in `declare_id!` and `Anchor.toml`; it is not evidence of a public deployment.

## Program instructions

| Instruction | Arguments | Behavior |
| --- | --- | --- |
| `create_collection` | `name: String`, `uri: String` | Creates a Core collection controlled by its update-authority PDA. |
| `mint_asset` | `name: String`, `uri: String` | Creates a Core asset in the collection, owned by the signing user. |
| `initialize` | `rewards_bps: u16`, `freeze_period: u16` | Creates the configuration and six-decimal reward mint for a collection. |
| `stake` | None | Validates ownership and collection membership, writes staking attributes, and freezes the asset. |
| `unstake` | None | Checks staking duration, resets attributes, thaws the asset, and mints rewards. |

The TypeScript client exposes camel-case names such as `createCollection` and `mintAsset`.

### Collection creation and minting

`create_collection` requires the payer and a new collection keypair to sign. The collection update authority is derived by the staking program.

`mint_asset` requires the user and a new asset keypair to sign. The program signs as the collection update-authority PDA for its Core CPI, and assigns ownership to the user.

The `name` and `uri` arguments are passed to Core. Metadata hosting and uploading are outside this program. The tests use example URLs.

### Initialization

Initialization creates one configuration and one reward mint per collection. The configuration uses Anchor's `init`, so the same configuration cannot be initialized twice.

Although the payer account is named `admin`, the current code does not authenticate a designated administrator or store an admin address. Any signer can initialize an eligible, uninitialized collection and select its parameters.

### Staking

The program checks that:

- The signer owns the asset.
- The asset's update authority identifies the supplied collection.
- The collection has the expected program-derived update authority.
- The configuration belongs to that collection.
- An existing `staked` attribute, if present, is `"false"`.

It adds or updates the Attributes plugin with:

```text
staked = "true"
staked_at = "<Solana Clock Unix timestamp in seconds>"
```

It then adds a FreezeDelegate plugin with `frozen = true`. The owner authorizes adding this owner-managed plugin; the program assigns its plugin authority to the update authority so the program can later thaw the asset.

### Unstaking

The program reads the staking attributes and calculates completed staking days from the Solana Clock. After the configured minimum duration has elapsed, it:

1. Resets `staked` to `"false"` and `staked_at` to `"0"`.
2. Updates the FreezeDelegate plugin to `frozen = false`.
3. Creates the owner's reward ATA if it does not exist.
4. Mints rewards using the configuration PDA as mint authority.

These changes occur within one instruction. If a CPI fails, the transaction's changes are rolled back.

There is no callable `claim_rewards` instruction. Rewards are minted only during unstaking; the `claim_rewards.rs` file is empty and its module exports are commented out.

## Reward calculation

```text
completed_days = (current_timestamp - staked_at) / 86,400
reward_base_units = completed_days × rewards_bps × 10^decimals / 10,000
```

For normal nonnegative elapsed time, integer division discards any partial day. The minimum duration check compares `completed_days` against `freeze_period`.

With the configured six-decimal mint:

| `rewards_bps` | Reward per completed day | Reward after 8 completed days |
| --- | --- | --- |
| `10,000` | 1 token | 8 tokens |
| `5,000` | 0.5 tokens | 4 tokens |
| `1,000` | 0.1 tokens | 0.8 tokens |

Despite its name, `rewards_bps` is a daily token-emission multiplier: the calculation has no NFT valuation or principal amount. It does not calculate an APR or percentage return on the NFT's value.

Rewards use the entire completed staking duration, not just time beyond the minimum freeze period. A seven-day minimum permits unstaking after seven completed days; it does not cap rewards at seven days.

The configuration stores `u16` values without explicit parameter-range validation. `InvalidRewardsBps` currently reports reward arithmetic failures rather than rejecting an out-of-range value during initialization.

## Accounts and PDA seeds

All PDAs below are derived under the staking program ID.

| Account | Seeds | Purpose |
| --- | --- | --- |
| Update authority | `[b"update_authority", collection_pubkey]` | Controls the collection and signs Core plugin updates. |
| Configuration | `[b"config", collection_pubkey]` | Stores staking parameters and PDA bumps; authorizes reward minting. |
| Reward mint | `[b"rewards_mint", config_pubkey]` | Fungible reward token mint with six decimals. |
| Owner reward ATA | Standard ATA derivation for owner and reward mint | Receives rewards on unstaking. |

The update-authority PDA is used for signing and is not initialized as a separate Anchor state account. The collection and NFT asset addresses come from newly generated keypairs, not staking-program PDA seeds.

### Configuration state

```rust
pub struct Config {
    pub rewards_bps: u16,
    pub freeze_period: u16,
    pub rewards_bump: u8,
    pub bump: u8,
}
```

`freeze_period` is measured in days. There is no stored administrator, total-staked counter, reward treasury, per-owner staking balance, or configurable emission cap.

## Repository structure

| Path | Contents |
| --- | --- |
| `programs/anchor-core-staking/src/lib.rs` | Program ID and instruction entry points |
| `programs/anchor-core-staking/src/state/config.rs` | Collection staking configuration |
| `programs/anchor-core-staking/src/instructions/` | Collection, minting, initialization, staking, and unstaking handlers |
| `programs/anchor-core-staking/src/error.rs` | Custom program errors |
| `tests/anchor-core-staking.ts` | Sequential integration flow and Surfpool time-travel helper |
| `Anchor.toml` | Anchor toolchain, local program ID, provider, and test script |
| `runbooks/deployment/` | Surfpool deployment runbook and environment signers |
| `txtx.yml` | Runbook registration and environment configuration |

## Getting started

### Prerequisites

Install Rust, the Solana CLI, Anchor CLI `0.31.1`, Node.js, Yarn, and Surfpool. Configure a local Solana wallet at `~/.config/solana/id.json`, or update the provider wallet path in `Anchor.toml`.

The repository does not pin Node.js, Solana CLI, or Surfpool versions. Consult their official installation documentation for compatible versions.

### Clone and build

```bash
git clone https://github.com/Gidy515/anchor-core-staking.git
cd anchor-core-staking
yarn install
anchor build
```

Building generates the program binary, IDL, and TypeScript types under `target/`.

For a fresh clone without the original deployment keypair, synchronize program IDs and rebuild:

```bash
anchor keys sync
anchor build
```

Confirm the resulting ID matches both `declare_id!` and the localnet entry in `Anchor.toml`.

### Start the local network

In one terminal, from the repository root:

```bash
surfpool start
```

Use the included deployment runbook if the program has not already been deployed by your Surfpool startup workflow:

```bash
surfpool run deployment
```

Ensure the local wallet is funded. If needed:

```bash
solana airdrop 10 --url http://127.0.0.1:8899
```

Metaplex Core must also be available on the local network at `MPL_CORE_PROGRAM_ID`. The checked-in Anchor configuration does not explicitly load a Core binary. Ensure your Surfpool setup can resolve the Core program and its required accounts.

### Run integration tests

With Surfpool running and the staking program deployed:

```bash
anchor test --skip-local-validator --skip-deploy
```

The tests invoke the Surfpool-specific `surfnet_timeTravel` RPC method. A plain `solana-test-validator` cannot run the time-travel step unchanged.

The test helper sends `absoluteTimestamp` in milliseconds using `Date.now()`, while the on-chain program reads Clock timestamps in seconds. If your Surfpool version has a different RPC contract, adjust the test helper to match it.

## Test coverage

The checked-in suite exercises this sequence:

1. Create a Core collection.
2. Mint a Core NFT into it.
3. Initialize with `rewards_bps = 10,000` and a seven-day minimum.
4. Stake the NFT.
5. Confirm early unstaking fails with `FreezePeriodNotElapsed`.
6. Advance local-network time by eight days through Surfpool.
7. Unstake and print the owner's reward-token balance.

Most steps validate transaction success and log results. The suite does not explicitly assert the reward amount, plugin state, or ownership after each transition. It also does not cover repeated staking cycles or most invalid-account cases.

## Custom errors

| Error | Meaning |
| --- | --- |
| `InvalidOwner` | The signing owner does not match the asset owner. |
| `InvalidUpdateAuthority` | The asset or collection does not satisfy the required authority relationship. |
| `AlreadyStaked` | An existing staking flag is not `"false"` when staking is requested. |
| `AssetNotStaked` | Attributes are absent, or an encountered staking flag is not `"true"` when unstaking. |
| `InvalidTimestamp` | Timestamp parsing or checked timestamp arithmetic fails. |
| `FreezePeriodNotElapsed` | Completed staking days are below the configured minimum. |
| `InvalidRewardsBps` | Checked reward arithmetic fails. |

Core, Anchor, and token-program operations may also return their own errors.

## Current limitations

- **Initialization is permissionless.** No designated-admin check prevents another signer from choosing an uninitialized collection's parameters first.
- **Minting is permissionless through this instruction.** There is no allowlist, mint price, supply cap, or administrator gate in `mint_asset`; a user pays account-creation costs.
- **Repeated staking needs additional handling.** Unstaking leaves the FreezeDelegate plugin installed with `frozen = false`, but staking always attempts to add that plugin. A later stake can fail because the plugin already exists; handle updating the existing plugin to support repeated cycles.
- **Attribute validation is incomplete.** Unstaking checks encountered staking keys but does not explicitly require exactly one `staked` key and exactly one `staked_at` key. It also does not explicitly reject future staking timestamps. Validate the full attribute schema before relying on it.
- **Plugin-fetch errors are discarded.** Calls to `fetch_plugin(...).ok()` treat any fetch failure as absence rather than distinguishing a missing plugin from other failures.
- **Rewards are minted on demand.** There is no funded treasury, maximum token supply, or global emission budget in the program.
- **Parameter updates and emergency controls are absent.** There are no instructions for changing configuration, pausing staking, or emergency unstaking.
- **Token-program coverage is limited.** Reward accounts use Anchor token-interface types, but the checked-in integration tests use the classic SPL Token program. Token-2022 behavior is not established by this suite.
- **Deployment and execution require verification.** This README describes the source; it does not establish an audited deployment or a passing test run in a particular environment.

## References

- [Metaplex Core documentation](https://developers.metaplex.com/core)
- [Anchor documentation](https://www.anchor-lang.com/docs)
- [Surfpool documentation](https://docs.surfpool.run)
- [Solana documentation](https://solana.com/docs)

## License

`package.json` declares `ISC`. The reviewed repository does not include a standalone `LICENSE` file; add one to make the intended licensing explicit.