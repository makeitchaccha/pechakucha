use crate::command::Context;
use crate::command::Result;

#[poise::command(prefix_command)]
pub async fn register(ctx: Context<'_>) -> Result<()> {
    poise::builtins::register_application_commands_buttons(ctx).await?;
    Ok(())
}
