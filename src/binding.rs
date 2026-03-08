use std::sync::Arc;
use poise::serenity_prelude::{ChannelId, GuildId};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

/// Table schema:
/// guild_id -> (voice_channel_id, text_channel_id)
const BINDINGS_TABLE: TableDefinition<u64, (u64, u64)> = TableDefinition::new("bindings");

pub struct Binding {
    pub voice: ChannelId,
    pub text: ChannelId,
}

impl Binding {
    pub fn new(voice: ChannelId, text: ChannelId) -> Self {
        Self { voice, text }
    }

    pub fn from_tuple(tuple: (u64, u64)) -> Self {
        Self {
            voice: ChannelId::new(tuple.0),
            text: ChannelId::new(tuple.1),
        }
    }

    pub fn into_tuple(self) -> (u64, u64) {
        (self.voice.get(), self.text.get())
    }
}

pub struct BindingRepository {
    db: Arc<Database>
}

impl BindingRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    pub async fn find_binding(&self, guild: GuildId) -> anyhow::Result<Option<Binding>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(BINDINGS_TABLE)?;
        Ok(table.get(guild.get())?.map(|binding| Binding::from_tuple(binding.value())))
    }

    pub async fn save_binding(&self, guild: GuildId, binding: Binding) -> anyhow::Result<()> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let tx = db.begin_write()?;
            {
                let mut table = tx.open_table(BINDINGS_TABLE)?;
                table.insert(guild.get(), binding.into_tuple())?;
            }
            tx.commit()?;
            Ok(())
        }).await??;

        Ok(())
    }

    pub async fn delete_binding(&self, guild: GuildId) -> anyhow::Result<()> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let tx = db.begin_write()?;
            {
                let mut table = tx.open_table(BINDINGS_TABLE)?;
                table.remove(guild.get())?;
            }
            Ok(())
        }).await??;

        Ok(())
    }
}