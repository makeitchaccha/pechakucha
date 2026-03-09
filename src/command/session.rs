use crate::command::{Context, Result};
use crate::session::actor::SessionActor;
use crate::session::driver::SongbirdDriver;
use anyhow::Context as _;
use poise::CreateReply;
use poise::serenity_prelude::{ChannelId, CreateEmbed, Mentionable};
use std::sync::Arc;

fn user_voice_channel_id(ctx: &Context<'_>) -> Result<ChannelId> {
    let channel_id = ctx
        .guild()
        .ok_or_else(|| anyhow::anyhow!("Guild not found"))?
        .voice_states
        .get(&ctx.author().id)
        .and_then(|vs| vs.channel_id)
        .ok_or_else(|| {
            anyhow::anyhow!("You have to join a voice channel to start text-to-speech")
        })?;
    Ok(channel_id)
}

#[poise::command(slash_command, guild_only)]
pub async fn join(ctx: Context<'_>) -> Result<()> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("Guild only"))?;

    let channel_id = user_voice_channel_id(&ctx)?;

    let manager = songbird::get(ctx.serenity_context())
        .await
        .ok_or_else(|| anyhow::anyhow!("Songbird Voice client not initialized"))?
        .clone();

    let handler = manager
        .join(guild_id, channel_id)
        .await
        .context("Failed to join voice channel")?;

    // prepare session actor to start text-to-speech
    let driver = SongbirdDriver { call: handler };
    let (actor, handle) = SessionActor::new(Arc::new(driver));

    tokio::spawn(actor.run());

    ctx.data()
        .session_manager
        .register(guild_id, ctx.channel_id(), channel_id, handle.clone());

    let profile = ctx
        .data()
        .resolver
        .resolve_guild_with_fallback(guild_id)
        .await;

    let profile_str = match &profile {
        Ok(profile) => profile.id.as_str(),
        Err(_) => ctx.data().resolver.fallback(),
    };

    let voice = ctx.data().registry.get_voice(profile_str).unwrap();

    handle
        .announce(
            ctx.data()
                .tts_locales
                .resolve(voice.language(), "launch", None, None)?,
            voice,
        )
        .await?;

    let discord_locales = &ctx.data().discord_locales;
    let locale = ctx.locale().expect("must be some when slash command");
    ctx.send(
        CreateReply::default().embed(
            CreateEmbed::new()
                .title(discord_locales.resolve(locale, "join-response", None, None)?)
                .field(
                    discord_locales.resolve(
                        locale,
                        "join-response",
                        Some("reading-channel"),
                        None,
                    )?,
                    ctx.channel_id().mention().to_string(),
                    true,
                )
                .field(
                    discord_locales.resolve(
                        locale,
                        "join-response",
                        Some("voice-channel"),
                        None,
                    )?,
                    channel_id.mention().to_string(),
                    true,
                ),
        ),
    )
    .await?;

    Ok(())
}

#[poise::command(slash_command, guild_only)]
pub async fn leave(ctx: Context<'_>) -> Result<()> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("Guild only"))?;

    let session = ctx
        .data()
        .session_manager
        .get(guild_id)
        .ok_or(anyhow::anyhow!("Guild not found"))?;

    session.handle.leave().await?;

    let discord_locales = &ctx.data().discord_locales;
    let locale = ctx.locale().expect("must be some when slash command");
    ctx.send(
        CreateReply::default().embed(
            CreateEmbed::new()
                .title(discord_locales.resolve(locale, "leave-response", None, None)?)
                .description(discord_locales.resolve(
                    locale,
                    "leave-response",
                    Some("thanks"),
                    None,
                )?),
        ),
    )
    .await?;

    Ok(())
}
