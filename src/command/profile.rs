use crate::command::{Context, Result};
use anyhow::anyhow;
use poise::serenity_prelude::AutocompleteChoice;
use std::fmt::{Display, Formatter};

#[poise::command(
    slash_command,
    guild_only,
    subcommands("user", "guild"),
    subcommand_required
)]
pub async fn voice(_: Context<'_>) -> Result<()> {
    Ok(())
}

#[poise::command(
    slash_command,
    subcommands("user_choose", "user_clear"),
    subcommand_required
)]
pub async fn user(_ctx: Context<'_>) -> Result<()> {
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
    common_choose(ctx, name).await
}

/// Clear your reading voice
#[poise::command(slash_command, rename = "clear", identifying_name = "voice-user-clear")]
pub async fn user_clear(ctx: Context<'_>) -> Result<()> {
    common_clear(ctx).await
}

#[poise::command(
    slash_command,
    subcommands("guild_choose", "guild_clear"),
    subcommand_required
)]
pub async fn guild(_ctx: Context<'_>) -> Result<()> {
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
    common_choose(ctx, name).await
}

/// Clear guild default reading voice
#[poise::command(
    slash_command,
    rename = "clear",
    identifying_name = "voice-guild-clear"
)]
pub async fn guild_clear(ctx: Context<'_>) -> Result<()> {
    common_clear(ctx).await
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

fn find_scope(ctx: Context<'_>) -> Result<Scope> {
    let scope = ctx
        .parent_commands()
        .last()
        .ok_or(anyhow!("missing parent"))?;
    match scope.name.as_str() {
        "user" => Ok(Scope::User),
        "guild" => Ok(Scope::Guild),
        _ => anyhow::bail!("unknown option"),
    }
}

async fn autocomplete_voice_name(
    ctx: Context<'_>,
    partial: &str,
) -> impl Iterator<Item = AutocompleteChoice> {
    let candidates = ctx.data().registry.find_prefixed_all(partial);
    candidates.map(|(id, package)| {
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
}

pub async fn common_choose(ctx: Context<'_>, name: String) -> Result<()> {
    let scope = find_scope(ctx)?;

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

pub async fn common_clear(ctx: Context<'_>) -> Result<()> {
    let scope = find_scope(ctx)?;

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
