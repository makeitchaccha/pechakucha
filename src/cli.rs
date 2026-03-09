use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "text-to-speech-rs")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
    #[arg(long, default_value = "./config.toml", help = "Config file")]
    pub config: PathBuf,
    #[arg(long, default_value = "./data", help = "Data directory to save")]
    pub data_dir: PathBuf,
}

#[derive(Parser)]
pub enum Commands {
    Run {
        #[arg(
            long,
            default_value_t = true,
            help = "Automatically run database migrations on startup"
        )]
        auto_migrate: bool,
    },
    Migrate {
        #[command(subcommand)]
        command: MigrateCommand,
    },
}

#[derive(Parser)]
pub enum MigrateCommand {
    Up,
    Status,
}
