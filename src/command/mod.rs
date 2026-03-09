mod link;
mod moderation;
mod profile;
mod session;

pub fn commands() -> Vec<poise::Command<crate::handler::Data, Error>> {
    vec![
        session::join(),
        session::leave(),
        link::link(),
        link::unlink(),
        moderation::register(),
        profile::voice(),
    ]
}

pub type Error = anyhow::Error;
pub type Result<T> = std::result::Result<T, Error>;
pub type Context<'a> = poise::Context<'a, crate::handler::Data, Error>;
