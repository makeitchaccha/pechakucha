mod cli;
mod database;

use std::path::PathBuf;
use std::sync::Arc;
use anyhow::{anyhow, Context};
use google_cloud_texttospeech_v1::client::TextToSpeech;
use poise::serenity_prelude as serenity;
use poise::serenity_prelude::GatewayIntents;
use songbird::SerenityInit;
use clap::{Parser};
use redb::Database;
use text_to_speech_rs::config::{load_config, AppConfig, DatabaseConfig, DatabaseKind};
use text_to_speech_rs::handler::event_handler;
use text_to_speech_rs::localization::{load_discord_locales, load_tts_locales};
use text_to_speech_rs::profile::resolver::ProfileResolver;
use text_to_speech_rs::session::manager::SessionManager;
use text_to_speech_rs::tts::registry::VoicePackageRegistry;
use text_to_speech_rs::{command, handler};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use text_to_speech_rs::binding::BindingRepository;
use crate::cli::MigrateCommand;
use crate::database::WrappedPool;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow!("CryptoProvider was already installed"))?;
    info!("Initialized CryptoProvider");

    if !cli.data_dir.exists() {
        std::fs::create_dir_all(&cli.data_dir)
            .context("Failed to create data directory")?;
    }

    let config = load_config(cli.config.as_path())
        .with_context(|| format!("Failed to load config from {}", cli.config.display()))?;

    let pool = prepare_database(&config.database).await?;

    match cli.command {
        cli::Commands::Run { auto_migrate } => {
            cli_run(config, &cli.data_dir, pool, auto_migrate).await
        },
        cli::Commands::Migrate { command } => {
            match command {
                MigrateCommand::Up => {
                    pool.migrate_up().await?;
                    Ok(())
                },
                MigrateCommand::Status => {
                    let status = pool.migrate_status().await?;
                    for (m, is_applied) in status {
                        let mark = if is_applied { "✅" } else { "⚠️ PENDING" };
                        println!("{} [{}] {}", mark, m.version, m.description);
                    }
                    Ok(())
                }
            }
        },
    }
}

async fn cli_run(config: AppConfig, data_dir: &PathBuf, pool: WrappedPool, auto_migrate: bool) -> anyhow::Result<()> {
    if auto_migrate {
        pool.migrate_up().await?;
    } else {
        // just check only.
        let status = pool.migrate_status().await?;
        let pending_count = status.iter().filter(|(_, is_applied)| !*is_applied).count();

        if pending_count > 0 {
            // then there is a pending migration.
            error!("Database schema is out of date ({} pending migrations).", pending_count);
            error!("Details:");
            for (m, _) in status.iter().filter(|(_, is_applied)| !*is_applied) {
                error!("⚠️ PENDING [{}] {}", m.version, m.description);
            }
            anyhow::bail!("Please run '{} migrate up' or start with '--auto-migrate=true'.", std::env::args().next().unwrap_or("bot".to_string()));
        }
    }

    let tts_locales = load_tts_locales("en")?;
    let discord_locales = load_discord_locales("en-US")?;

    info!("Starting text-to-speech bot");

    config.verify()?;

    info!("Loaded config");

    let mut registry_builder = VoicePackageRegistry::builder(config.clone());

    if let Some(_google_cloud_config) = &config.backend.google_cloud {
        info!("Using Google Cloud credentials");
        let client = TextToSpeech::builder().build().await?;

        registry_builder = registry_builder.google_cloud(client);
    }

    let registry = registry_builder
        .build()
        .context("Failed to build VoiceRegistry")?;

    info!("VoiceRegistry built successfully.");

    let data_db_path = data_dir.join("data.redb");
    let data_db = Arc::new(Database::create(data_db_path)?);
    let binding_repository = BindingRepository::new(data_db);
    info!("Loaded bindings");

    let profile_repository = pool.profile_repository();

    let resolver = ProfileResolver::new(profile_repository.clone(), config.bot.global_profile.clone());

    let mut commands = command::commands();

    discord_locales.apply(&mut commands)?;

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands,
            event_handler: |ctx, event, framework, data| {
                Box::pin(event_handler(ctx, event, framework, data))
            },
            ..Default::default()
        })
        .setup(move |_ctx, _ready, _framework| {
            Box::pin(async move {
                Ok(handler::Data {
                    session_manager: SessionManager::new(),
                    registry,
                    resolver,
                    repository: profile_repository,
                    tts_locales,
                    discord_locales,
                    binding_repository,
                })
            })
        })
        .build();

    let intents = GatewayIntents::non_privileged() | GatewayIntents::MESSAGE_CONTENT;
    let mut client = serenity::ClientBuilder::new(config.bot.token, intents)
        .register_songbird()
        .framework(framework).await?;

    client.start().await?;

    Ok(())
}

async fn prepare_database(config: &DatabaseConfig) -> anyhow::Result<WrappedPool> {
    match config.kind {
        DatabaseKind::SQLite => {
            #[cfg(feature = "sqlite")]
            {
                info!("Opening SQLite database...");
                Ok(WrappedPool::Sqlite(sqlx::SqlitePool::connect(&config.url).await?))
            }
            #[cfg(not(feature = "sqlite"))]
            anyhow::bail!("SQLite selected, but 'sqlite' feature is not enabled.")
        },
        DatabaseKind::Postgres => {
            #[cfg(feature = "postgres")]
            {
                info!("Connecting to PostgreSQL...");
                Ok(WrappedPool::Postgres(sqlx::PgPool::connect(&config.url).await?))
            }
            #[cfg(not(feature = "postgres"))]
            anyhow::bail!("PostgreSQL selected, but 'postgres' feature is not enabled.")
        }
    }
}
