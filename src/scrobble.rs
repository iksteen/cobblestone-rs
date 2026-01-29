use anyhow::Result;

use crate::rockbox::{PlaybackEntry, TagCache};

pub const MIN_TRACK_SECONDS: i64 = 30;

#[derive(Debug, Clone)]
pub struct ScrobbleTrack {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub timestamp: i64,
    pub duration: i64,
}

pub fn build_scrobble_tracks(
    playback_entries: &[PlaybackEntry],
    tagcache: &mut TagCache,
) -> Result<(Vec<ScrobbleTrack>, Vec<String>)> {
    let mut tracks = Vec::new();
    let mut missing = Vec::new();
    for entry in playback_entries {
        if !is_scrobble_eligible(entry) {
            continue;
        }
        let info = tagcache.get_track_info(&entry.path)?;
        let Some(info) = info else {
            missing.push(entry.path.clone());
            continue;
        };
        tracks.push(ScrobbleTrack {
            artist: info.artist,
            title: info.title,
            album: info.album,
            timestamp: entry.timestamp,
            duration: info.duration_seconds,
        });
    }
    Ok((tracks, missing))
}

fn is_scrobble_eligible(entry: &PlaybackEntry) -> bool {
    if entry.total_ms <= 0 {
        return false;
    }
    let total_seconds = entry.total_ms / 1000;
    if total_seconds < MIN_TRACK_SECONDS {
        return false;
    }
    let min_played_ms = (entry.total_ms / 2).min(240_000);
    entry.elapsed_ms >= min_played_ms
}
