use crate::source_resolver::SourceResolver;
use music_player_settings::read_settings;
use music_player_storage::Database;

impl SourceResolver {
    /// All daemon/API entry points use the same redirect policy and device ID.
    pub fn from_settings(db: Database) -> Self {
        let settings = read_settings().ok();
        let device_id = settings
            .as_ref()
            .and_then(|s| s.get_string("device_id").ok())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let follow_redirects = settings
            .as_ref()
            .and_then(|s| s.get_bool("emby_follow_redirects").ok())
            .unwrap_or(true);
        Self::new(db, device_id, follow_redirects)
    }
}
