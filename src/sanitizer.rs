use anyhow::anyhow;
use poise::serenity_prelude as serenity;
use poise::serenity_prelude::{ChannelMention, GuildId, Mentionable, RoleId, User};
use regex::Regex;
use std::sync::LazyLock;

static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+").unwrap());
static EMOJI_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<a?:(\w+):\d+>").unwrap());
static CODE_BLOCK_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```(?:\w*\n)?(.*?)```").unwrap());

pub fn replace_mentions(
    content: &str,
    ctx: &serenity::Context,
    guild_id: GuildId,
    users: &[User],
    roles: &[RoleId],
    channels: &[ChannelMention],
) -> anyhow::Result<String> {
    let mut content = content.to_string();

    let guild = guild_id
        .to_guild_cached(&ctx.cache)
        .ok_or(anyhow!("Guild not found"))?;

    for user in users {
        content = content.replace(
            user.mention().to_string().as_str(),
            guild
                .members
                .get(&user.id)
                .ok_or(anyhow!("User does not exist"))?
                .display_name(),
        );
    }
    for role in roles {
        content = content.replace(
            role.mention().to_string().as_str(),
            &guild
                .roles
                .get(role)
                .ok_or(anyhow!("Failed to get roles from cache"))?
                .name,
        );
    }
    for channel in channels {
        content = content.replace(channel.id.mention().to_string().as_str(), &channel.name);
    }

    Ok(content)
}

pub fn sanitize(content: &str, limit: usize) -> String {
    let mut content = CODE_BLOCK_REGEX
        .replace_all(content, "code block")
        .to_string();

    content = URL_REGEX.replace_all(&content, "URL").to_string();

    content = EMOJI_REGEX.replace_all(&content, "EMOJI").to_string();

    content.chars().take(limit).collect()
}
