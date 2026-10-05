use poise::serenity_prelude as serenity;
use poise::serenity_prelude::{ChannelMention, GuildId, Mentionable, RoleId, User};
use regex::Regex;
use std::sync::LazyLock;
use tracing::warn;

static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+").unwrap());
static EMOJI_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<a?:(\w+):\d+>").unwrap());
static CODE_BLOCK_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?ms)```(?:\w*\n)?(.*?)```").unwrap());

pub fn normalize_mentions(
    content: &str,
    ctx: &serenity::Context,
    guild_id: GuildId,
    users: &[User],
    roles: &[RoleId],
    channels: &[ChannelMention],
) -> String {
    let mut content = content.to_string();

    let Some(guild) = guild_id.to_guild_cached(&ctx.cache) else {
        warn!("Failed to replace mentions: no guild found in cache.");

        return content;
    };

    for user in users {
        content = content.replace(
            user.mention().to_string().as_str(),
            guild
                .members
                .get(&user.id)
                .map(|member| member.display_name())
                .unwrap_or(user.display_name()),
        );
    }
    for role_id in roles {
        content = content.replace(
            role_id.mention().to_string().as_str(),
            guild
                .roles
                .get(role_id)
                .map(|role| role.name.as_str())
                .unwrap_or("role"),
        );
    }
    for channel in channels {
        content = content.replace(
            channel.id.mention().to_string().as_str(),
            channel.name.as_str(),
        );
    }

    content
}

pub fn normalize_urls(content: &str) -> String {
    URL_REGEX.replace_all(content, "URL").to_string()
}

pub fn normalize_emojis(content: &str) -> String {
    EMOJI_REGEX.replace_all(content, "EMOJI").to_string()
}

pub fn normalize_code_blocks(content: &str) -> String {
    CODE_BLOCK_REGEX
        .replace_all(content, "code block")
        .to_string()
}

fn normalize_line_breaks(content: &str) -> String {
    let mut normalized = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();

    while let Some(character) = chars.next() {
        if character == '\n' || character == '\r' {
            if character == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }

            let trimmed_len = normalized.trim_end().len();
            normalized.truncate(trimmed_len);
            let already_terminated = normalized.chars().last().is_some_and(|last| {
                matches!(
                    last,
                    '。' | '！' | '？' | '.' | '!' | '?' | '、' | '，' | ',' | '；' | ';' | '…'
                )
            });
            if !normalized.is_empty() && !already_terminated {
                normalized.push('.');
            }
        } else {
            normalized.push(character);
        }
    }

    normalized
}

pub fn preprocess(content: &str, limit: usize) -> String {
    let content = normalize_code_blocks(content);
    let content = normalize_urls(&content);
    let content = normalize_emojis(&content);
    let content = normalize_line_breaks(&content);

    content.chars().take(limit).collect()
}
