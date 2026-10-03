use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{
        mint_to_checked, Mint, MintToChecked, TokenAccount, TokenInterface,
    },
};
use mpl_core::{
    accounts::{BaseAssetV1, BaseCollectionV1},
    fetch_plugin,
    instructions::UpdatePluginV1CpiBuilder,
    types::{
        Attribute, Attributes, FreezeDelegate, Plugin, PluginType,
        UpdateAuthority,
    },
    ID as MPL_CORE_ID,
};

use crate::{error::ErrorCode, Config};

const SECONDS_PER_DAY: i64 = 86_400;

pub(crate) const CLAIMED_REWARDS: &str = "claimed_rewards";

#[derive(Accounts)]
pub struct ClaimRewards<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(
        seeds = [b"config", collection.key().as_ref()],
        bump = config.bump,
    )]
    pub config: Account<'info, Config>,

    #[account(
        mut,
        has_one = owner @ ErrorCode::InvalidOwner,
        constraint = asset.update_authority
            == UpdateAuthority::Collection(collection.key())
            @ ErrorCode::InvalidUpdateAuthority,
    )]
    pub asset: Account<'info, BaseAssetV1>,

    #[account(
        mut,
        has_one = update_authority
            @ ErrorCode::InvalidUpdateAuthority,
    )]
    pub collection: Account<'info, BaseCollectionV1>,

    /// CHECK: Address validated using the collection-specific PDA seeds.
    #[account(
        seeds = [b"update_authority", collection.key().as_ref()],
        bump,
    )]
    pub update_authority: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [b"rewards_mint", config.key().as_ref()],
        bump = config.rewards_bump,
    )]
    pub rewards_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init_if_needed,
        payer = owner,
        associated_token::mint = rewards_mint,
        associated_token::authority = owner,
        associated_token::token_program = token_program,
    )]
    pub user_rewards_ata: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,

    /// CHECK: Address restricted to the Metaplex Core program.
    #[account(address = MPL_CORE_ID)]
    pub mpl_core_program: UncheckedAccount<'info>,
}

// Reads an attribute while rejecting duplicate accounting keys.
fn unique_attribute<'a>(
    attributes: &'a Attributes,
    key: &str,
) -> Result<Option<&'a str>> {
    let mut matches = attributes
        .attribute_list
        .iter()
        .filter(|attribute| attribute.key == key);

    let value = matches.next().map(|attribute| attribute.value.as_str());

    require!(
        matches.next().is_none(),
        ErrorCode::InvalidStakingAttributes
    );

    Ok(value)
}

// Returns:
// (completed staking days, total earned base units, unclaimed base units)
//
// Both claiming and unstaking use this function to avoid double payment.
pub(crate) fn reward_entitlement(
    attributes: &Attributes,
    now: i64,
    rewards_bps: u16,
    decimals: u8,
) -> Result<(u64, u64, u64)> {
    require!(
        unique_attribute(attributes, "staked")? == Some("true"),
        ErrorCode::AssetNotStaked
    );

    let staked_at = unique_attribute(attributes, "staked_at")?
        .ok_or(ErrorCode::InvalidStakingAttributes)?
        .parse::<i64>()
        .map_err(|_| ErrorCode::InvalidTimestamp)?;

    require!(
        staked_at >= 0 && now >= staked_at,
        ErrorCode::InvalidTimestamp
    );

    let elapsed = now
        .checked_sub(staked_at)
        .ok_or(ErrorCode::InvalidTimestamp)?;

    let completed_days = (elapsed / SECONDS_PER_DAY) as u64;

    let scale = 10u64
        .checked_pow(decimals as u32)
        .ok_or(ErrorCode::InvalidRewardsBps)?;

    let total_earned = completed_days
        .checked_mul(rewards_bps as u64)
        .and_then(|value| value.checked_mul(scale))
        .and_then(|value| value.checked_div(10_000))
        .ok_or(ErrorCode::InvalidRewardsBps)?;

    // Existing staked assets without this attribute have claimed zero.
    let already_claimed = unique_attribute(attributes, CLAIMED_REWARDS)?
        .unwrap_or("0")
        .parse::<u64>()
        .map_err(|_| ErrorCode::InvalidStakingAttributes)?;

    let pending = total_earned
        .checked_sub(already_claimed)
        .ok_or(ErrorCode::InvalidStakingAttributes)?;

    Ok((completed_days, total_earned, pending))
}

pub fn handler(ctx: Context<ClaimRewards>) -> Result<()> {
    let (_, mut attributes, _) =
        fetch_plugin::<BaseAssetV1, Attributes>(
            &ctx.accounts.asset.to_account_info(),
            PluginType::Attributes,
        )
        .map_err(|_| ErrorCode::AssetNotStaked)?;

    // Claiming requires the NFT to remain frozen.
    let (_, freeze_delegate, _) =
        fetch_plugin::<BaseAssetV1, FreezeDelegate>(
            &ctx.accounts.asset.to_account_info(),
            PluginType::FreezeDelegate,
        )
        .map_err(|_| ErrorCode::AssetNotStaked)?;

    require!(
        freeze_delegate.frozen,
        ErrorCode::AssetNotStaked
    );

    let (_, total_earned, amount) = reward_entitlement(
        &attributes,
        Clock::get()?.unix_timestamp,
        ctx.accounts.config.rewards_bps,
        ctx.accounts.rewards_mint.decimals,
    )?;

    require!(amount > 0, ErrorCode::NoRewardsAvailable);

    // Preserve all other attributes, including the original staked_at.
    attributes
        .attribute_list
        .retain(|attribute| attribute.key != CLAIMED_REWARDS);

    attributes.attribute_list.push(Attribute {
        key: CLAIMED_REWARDS.to_string(),
        value: total_earned.to_string(),
    });

    let collection_key = ctx.accounts.collection.key();
    let authority_bump = [ctx.bumps.update_authority];

    let authority_seeds: &[&[u8]] = &[
        b"update_authority",
        collection_key.as_ref(),
        &authority_bump,
    ];

    // Update reward accounting only.
    // staked remains true and FreezeDelegate remains frozen.
    UpdatePluginV1CpiBuilder::new(
        &ctx.accounts.mpl_core_program.to_account_info(),
    )
    .asset(&ctx.accounts.asset.to_account_info())
    .collection(Some(&ctx.accounts.collection.to_account_info()))
    .payer(&ctx.accounts.owner.to_account_info())
    .authority(Some(
        &ctx.accounts.update_authority.to_account_info(),
    ))
    .system_program(&ctx.accounts.system_program.to_account_info())
    .plugin(Plugin::Attributes(attributes))
    .invoke_signed(&[authority_seeds])?;

    let config_bump = [ctx.accounts.config.bump];

    let config_seeds: &[&[u8]] = &[
        b"config",
        collection_key.as_ref(),
        &config_bump,
    ];

    mint_to_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintToChecked {
                mint: ctx.accounts.rewards_mint.to_account_info(),
                to: ctx.accounts.user_rewards_ata.to_account_info(),
                authority: ctx.accounts.config.to_account_info(),
            },
            &[config_seeds],
        ),
        amount,
        ctx.accounts.rewards_mint.decimals,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staking_attributes(
        start: i64,
        claimed: Option<u64>,
    ) -> Attributes {
        let mut attribute_list = vec![
            Attribute {
                key: "staked".into(),
                value: "true".into(),
            },
            Attribute {
                key: "staked_at".into(),
                value: start.to_string(),
            },
        ];

        if let Some(amount) = claimed {
            attribute_list.push(Attribute {
                key: CLAIMED_REWARDS.into(),
                value: amount.to_string(),
            });
        }

        Attributes { attribute_list }
    }

    #[test]
    fn claiming_then_unstaking_pays_only_remaining_rewards() {
        let start = 100;

        let first_claim = reward_entitlement(
            &staking_attributes(start, None),
            start + 2 * SECONDS_PER_DAY,
            10_000,
            6,
        )
        .unwrap();

        assert_eq!(first_claim, (2, 2_000_000, 2_000_000));

        let repeated_claim = reward_entitlement(
            &staking_attributes(start, Some(2_000_000)),
            start + 2 * SECONDS_PER_DAY,
            10_000,
            6,
        )
        .unwrap();

        assert_eq!(repeated_claim.2, 0);

        let unstake_rewards = reward_entitlement(
            &staking_attributes(start, Some(2_000_000)),
            start + 8 * SECONDS_PER_DAY,
            10_000,
            6,
        )
        .unwrap();

        assert_eq!(unstake_rewards.2, 6_000_000);
    }

    #[test]
    fn partial_days_remain_anchored_to_original_stake() {
        let start = 100;

        let before_one_day = reward_entitlement(
            &staking_attributes(start, None),
            start + SECONDS_PER_DAY - 1,
            5_000,
            6,
        )
        .unwrap();

        assert_eq!(before_one_day.2, 0);

        let after_two_days = reward_entitlement(
            &staking_attributes(start, Some(500_000)),
            start + 2 * SECONDS_PER_DAY + 500,
            5_000,
            6,
        )
        .unwrap();

        assert_eq!(after_two_days.2, 500_000);
    }

    #[test]
    fn invalid_accounting_state_is_rejected() {
        assert!(reward_entitlement(
            &staking_attributes(100, None),
            99,
            10_000,
            6,
        )
        .is_err());

        assert!(reward_entitlement(
            &staking_attributes(100, Some(2_000_000)),
            100 + SECONDS_PER_DAY,
            10_000,
            6,
        )
        .is_err());

        let mut duplicate = staking_attributes(100, None);

        duplicate.attribute_list.push(Attribute {
            key: "staked_at".into(),
            value: "100".into(),
        });

        assert!(reward_entitlement(
            &duplicate,
            100 + SECONDS_PER_DAY,
            10_000,
            6,
        )
        .is_err());

        let mut missing = staking_attributes(100, None);

        missing
            .attribute_list
            .retain(|attribute| attribute.key != "staked");

        assert!(reward_entitlement(
            &missing,
            100 + SECONDS_PER_DAY,
            10_000,
            6,
        )
        .is_err());
    }
}