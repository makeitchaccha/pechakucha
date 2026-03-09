use crate::binding::Binding;
use crate::command::{Context, Result};
use poise::CreateReply;
use poise::serenity_prelude::{CreateEmbed, GuildChannel, Mentionable};

#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn link(
    ctx: Context<'_>,
    #[channel_types("Voice")] voice_channel: GuildChannel,
) -> Result<()> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("Guild only"))?;

    let text_channel_id = ctx.channel_id();
    let voice_channel_id = voice_channel.id;
    ctx.data()
        .binding_repository
        .save_binding(guild_id, Binding::new(voice_channel_id, text_channel_id))
        .await?;

    let discord_locales = &ctx.data().discord_locales;
    let locale = ctx.locale().expect("must be some when slash command");
    ctx.send(
        CreateReply::default().embed(
            CreateEmbed::new()
                .title(discord_locales.resolve(locale, "link-response", None, None)?)
                .description(discord_locales.resolve(
                    locale,
                    "link-response",
                    Some("description"),
                    None,
                )?)
                .field(
                    discord_locales.resolve(
                        locale,
                        "link-response",
                        Some("reading-channel"),
                        None,
                    )?,
                    text_channel_id.mention().to_string(),
                    true,
                )
                .field(
                    discord_locales.resolve(
                        locale,
                        "link-response",
                        Some("voice-channel"),
                        None,
                    )?,
                    voice_channel_id.mention().to_string(),
                    true,
                ),
        ),
    )
    .await?;

    Ok(())
}

#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn unlink(ctx: Context<'_>) -> Result<()> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("Guild only"))?;

    ctx.data()
        .binding_repository
        .delete_binding(guild_id)
        .await?;

    let discord_locales = &ctx.data().discord_locales;
    let locale = ctx.locale().expect("must be some when slash command");
    ctx.send(
        CreateReply::default().embed(
            CreateEmbed::new()
                .title(discord_locales.resolve(locale, "unlink-response", None, None)?)
                .description(discord_locales.resolve(
                    locale,
                    "unlink-response",
                    Some("description"),
                    None,
                )?),
        ),
    )
    .await?;

    Ok(())
}
