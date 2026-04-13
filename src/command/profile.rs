use crate::command::{Context, Result};
use anyhow::anyhow;
use poise::serenity_prelude::AutocompleteChoice;
use std::fmt::{Display, Formatter};

/// Setting commands for user
#[poise::command(
    slash_command,
    guild_only,
    subcommands("user_choose", "user_clear"),
    subcommand_required
)]
pub async fn voice(_: Context<'_>) -> Result<()> {
    Ok(())
}

/// Choose your reading voice
#[poise::command(
    slash_command,
    rename = "choose",
    identifying_name = "voice-user-choose"
)]
pub async fn user_choose(
    ctx: Context<'_>,
    #[autocomplete = "autocomplete_voice_name"] name: String,
) -> Result<()> {
    common_choose(ctx, Scope::User, name).await
}

/// Clear your reading voice
#[poise::command(slash_command, rename = "clear", identifying_name = "voice-user-clear")]
pub async fn user_clear(ctx: Context<'_>) -> Result<()> {
    common_clear(ctx, Scope::User).await
}

#[poise::command(
    slash_command,
    guild_only,
    rename = "guild-voice",
    subcommands("guild_choose", "guild_clear"),
    subcommand_required,
    default_member_permissions = "MANAGE_GUILD"
)]
pub async fn guild_voice(_ctx: Context<'_>) -> Result<()> {
    Ok(())
}

/// Choose guild default reading voice
#[poise::command(
    slash_command,
    rename = "choose",
    identifying_name = "voice-guild-choose"
)]
pub async fn guild_choose(
    ctx: Context<'_>,
    #[autocomplete = "autocomplete_voice_name"] name: String,
) -> Result<()> {
    common_choose(ctx, Scope::Guild, name).await
}

/// Clear guild default reading voice
#[poise::command(
    slash_command,
    rename = "clear",
    identifying_name = "voice-guild-clear"
)]
pub async fn guild_clear(ctx: Context<'_>) -> Result<()> {
    common_clear(ctx, Scope::Guild).await
}

enum Scope {
    User,
    Guild,
}

impl Display for Scope {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Scope::User => write!(f, "user"),
            Scope::Guild => write!(f, "guild"),
        }
    }
}

async fn autocomplete_voice_name(
    ctx: Context<'_>,
    partial: &str,
) -> impl Iterator<Item = AutocompleteChoice> {
    let keywords: Vec<&str> = partial.split_whitespace().filter(|s| *s != "|").collect();
    let candidates = ctx
        .data()
        .registry
        .find_matching_keywords(keywords.as_ref());

    candidates
        .map(|(id, package)| {
            AutocompleteChoice::new(
                match package.detail.description.as_ref() {
                    Some(description) => format!(
                        "{}  |  {} ({})",
                        package.detail.provider, package.detail.name, description
                    ),
                    None => format!("{}  |  {}", package.detail.provider, package.detail.name),
                },
                id,
            )
        })
        .take(25)
        .collect::<Vec<_>>()
        .into_iter()
}

async fn common_choose(ctx: Context<'_>, scope: Scope, name: String) -> Result<()> {
    if ctx.data().registry.get_voice(name.as_str()).is_none() {
        return Err(anyhow::anyhow!(format!("voice {} not found", name)));
    }

    match scope {
        Scope::User => {
            ctx.data()
                .repository
                .save_user(ctx.author().id, &name)
                .await?;
        }
        Scope::Guild => {
            ctx.data()
                .repository
                .save_guild(ctx.guild_id().ok_or(anyhow!("guild not found"))?, &name)
                .await?;
        }
    }

    ctx.say(format!("{} was chosen for {} voice.", name, scope))
        .await?;

    Ok(())
}

async fn common_clear(ctx: Context<'_>, scope: Scope) -> Result<()> {
    match scope {
        Scope::User => {
            ctx.data().repository.delete_user(ctx.author().id).await?;
        }
        Scope::Guild => {
            ctx.data()
                .repository
                .delete_guild(ctx.guild_id().ok_or(anyhow!("guild not found"))?)
                .await?;
        }
    }

    ctx.say(format!("voice was cleared for {} voice.", scope))
        .await?;

    Ok(())
}
