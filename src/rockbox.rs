use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use byteorder::{BigEndian, ByteOrder, LittleEndian};
use chrono::{Local, LocalResult, TimeZone, Utc};

const TAGCACHE_MAGIC: u32 = 0x5443_4810;

const TAG_ARTIST: usize = 0;
const TAG_ALBUM: usize = 1;
const TAG_TITLE: usize = 3;
const TAG_FILENAME: i32 = 4;
const TAG_LENGTH: usize = 14;

const TAG_COUNT: usize = 23;

const TAGCACHE_HEADER_SIZE: usize = 12;
const TAGFILE_ENTRY_HEADER_SIZE: usize = 8;
const PLAYBACK_LOG_PARTS: usize = 4;

#[derive(Debug, Clone)]
pub struct PlaybackEntry {
    pub timestamp: i64,
    pub elapsed_ms: i64,
    pub total_ms: i64,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub duration_seconds: i64,
}

#[derive(Debug, Clone, Copy)]
enum Endian {
    Little,
    Big,
}

impl Endian {
    fn read_u32(self, bytes: &[u8]) -> u32 {
        match self {
            Endian::Little => LittleEndian::read_u32(bytes),
            Endian::Big => BigEndian::read_u32(bytes),
        }
    }

    fn read_i32(self, bytes: &[u8]) -> i32 {
        match self {
            Endian::Little => LittleEndian::read_i32(bytes),
            Endian::Big => BigEndian::read_i32(bytes),
        }
    }
}

pub struct TagCache {
    rockbox_dir: PathBuf,
    master_path: PathBuf,
    endian: Endian,
    entry_size: usize,
    master_header_size: usize,
    path_index: Option<HashMap<String, i32>>,
    tag_files: HashMap<i32, File>,
}

impl TagCache {
    pub fn new(rockbox_dir: &Path) -> Result<Self> {
        let master_path = rockbox_dir.join("database_idx.tcd");
        if !master_path.exists() {
            bail!("Missing tagcache file: {}", master_path.display());
        }
        let endian = Self::detect_endian(&master_path)?;
        Ok(Self {
            rockbox_dir: rockbox_dir.to_path_buf(),
            master_path,
            endian,
            entry_size: (TAG_COUNT + 1) * 4,
            master_header_size: 24,
            path_index: None,
            tag_files: HashMap::new(),
        })
    }

    pub fn close(&mut self) {
        self.tag_files.clear();
    }

    fn detect_endian(path: &Path) -> Result<Endian> {
        let mut handle = File::open(path)
            .with_context(|| format!("Failed opening tagcache {}", path.display()))?;
        let mut magic_bytes = [0u8; 4];
        handle
            .read_exact(&mut magic_bytes)
            .context("Failed reading tagcache header")?;
        if LittleEndian::read_u32(&magic_bytes) == TAGCACHE_MAGIC {
            return Ok(Endian::Little);
        }
        if BigEndian::read_u32(&magic_bytes) == TAGCACHE_MAGIC {
            return Ok(Endian::Big);
        }
        bail!("Unrecognized tagcache magic in {}", path.display());
    }

    fn with_tag_file<T>(&mut self, tag: i32, f: impl FnOnce(&mut File) -> Result<T>) -> Result<T> {
        if !self.tag_files.contains_key(&tag) {
            let path = self.rockbox_dir.join(format!("database_{tag}.tcd"));
            let handle = File::open(&path)
                .with_context(|| format!("Failed opening tagcache {}", path.display()))?;
            self.tag_files.insert(tag, handle);
        }
        let handle = self.tag_files.get_mut(&tag).expect("tag file exists");
        f(handle)
    }

    fn read_header(endian: Endian, handle: &mut File) -> Result<(u32, u32, u32)> {
        let mut header = [0u8; TAGCACHE_HEADER_SIZE];
        handle
            .read_exact(&mut header)
            .context("Short read when parsing tagcache header")?;
        let magic = endian.read_u32(&header[0..4]);
        let data_size = endian.read_u32(&header[4..8]);
        let entry_count = endian.read_u32(&header[8..12]);
        Ok((magic, data_size, entry_count))
    }

    fn read_tag_string(&mut self, tag: i32, seek: i32) -> Result<Option<String>> {
        if seek <= 0 {
            return Ok(None);
        }
        let seek = u64::try_from(seek).context("Invalid tagcache seek offset")?;
        let endian = self.endian;
        self.with_tag_file(tag, |handle| {
            handle.seek(SeekFrom::Start(seek))?;
            let mut entry = [0u8; TAGFILE_ENTRY_HEADER_SIZE];
            if handle.read(&mut entry)? != TAGFILE_ENTRY_HEADER_SIZE {
                return Ok(None);
            }
            let tag_length = endian.read_u32(&entry[0..4]);
            if tag_length == 0 {
                return Ok(None);
            }
            let tag_length =
                usize::try_from(tag_length).context("Invalid tagcache string length")?;
            let mut data = vec![0u8; tag_length];
            handle.read_exact(&mut data)?;
            if data.is_empty() {
                return Ok(None);
            }
            let value = data.split(|byte| *byte == 0).next().unwrap_or_default();
            Ok(Some(String::from_utf8_lossy(value).to_string()))
        })
    }

    fn load_path_index(&mut self) -> Result<&HashMap<String, i32>> {
        if self.path_index.is_none() {
            let endian = self.endian;
            let index = self.with_tag_file(TAG_FILENAME, |handle| {
                handle.seek(SeekFrom::Start(0))?;
                let (magic, _data_size, entry_count) = Self::read_header(endian, handle)?;
                if magic != TAGCACHE_MAGIC {
                    bail!("Tagcache filename index has invalid header");
                }
                let mut index = HashMap::new();
                for _ in 0..entry_count {
                    let mut entry = [0u8; TAGFILE_ENTRY_HEADER_SIZE];
                    if handle.read(&mut entry)? != TAGFILE_ENTRY_HEADER_SIZE {
                        break;
                    }
                    let tag_length = endian.read_u32(&entry[0..4]);
                    let idx_id = endian.read_u32(&entry[4..8]);
                    if tag_length == 0 {
                        continue;
                    }
                    let tag_length =
                        usize::try_from(tag_length).context("Invalid tagcache string length")?;
                    let idx_id = i32::try_from(idx_id).context("Invalid tagcache index id")?;
                    let mut data = vec![0u8; tag_length];
                    handle.read_exact(&mut data)?;
                    let path = data.split(|byte| *byte == 0).next().unwrap_or_default();
                    index.insert(String::from_utf8_lossy(path).to_string(), idx_id);
                }
                Ok(index)
            })?;
            self.path_index = Some(index);
        }
        Ok(self.path_index.as_ref().expect("path index initialized"))
    }

    pub fn find_idx_id(&mut self, path: &str) -> Result<Option<i32>> {
        Ok(self.load_path_index()?.get(path).copied())
    }

    pub fn get_track_info(&mut self, path: &str) -> Result<Option<TrackInfo>> {
        let Some(idx_id) = self.find_idx_id(path)? else {
            return Ok(None);
        };
        let entry = self.read_index_entry(idx_id)?;
        let artist = self.read_tag_string(tag_to_i32(TAG_ARTIST), entry[TAG_ARTIST])?;
        let title = self.read_tag_string(tag_to_i32(TAG_TITLE), entry[TAG_TITLE])?;
        let album = self.read_tag_string(tag_to_i32(TAG_ALBUM), entry[TAG_ALBUM])?;
        let artist = artist.unwrap_or_default();
        let title = title.unwrap_or_default();
        let duration_ms = i64::from(entry[TAG_LENGTH].max(0));
        let duration_seconds = duration_ms / 1000;
        if artist.is_empty() || title.is_empty() {
            return Ok(None);
        }
        Ok(Some(TrackInfo {
            artist,
            title,
            album: album.filter(|value| !value.is_empty()),
            duration_seconds,
        }))
    }

    fn read_index_entry(&self, idx_id: i32) -> Result<Vec<i32>> {
        if idx_id < 0 {
            bail!("Invalid tagcache index id {idx_id}");
        }
        let mut handle = File::open(&self.master_path)
            .with_context(|| format!("Failed opening tagcache {}", self.master_path.display()))?;
        let idx_id = u64::try_from(idx_id).context("Invalid tagcache index id")?;
        let header_size =
            u64::try_from(self.master_header_size).context("Invalid tagcache header size")?;
        let entry_size = u64::try_from(self.entry_size).context("Invalid tagcache entry size")?;
        let offset = header_size + (idx_id * entry_size);
        handle.seek(SeekFrom::Start(offset))?;
        let mut raw = vec![0u8; self.entry_size];
        handle
            .read_exact(&mut raw)
            .with_context(|| format!("Short read for index entry {idx_id}"))?;
        let mut values = Vec::with_capacity(TAG_COUNT + 1);
        for chunk in raw.chunks_exact(4) {
            values.push(self.endian.read_i32(chunk));
        }
        Ok(values)
    }
}

pub fn parse_playback_log(path: &Path) -> Result<Vec<PlaybackEntry>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed reading playback log {}", path.display()))?;
    let mut entries = Vec::new();
    for line in raw.lines() {
        let cleaned = line.trim();
        if cleaned.is_empty() || cleaned.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = cleaned.splitn(PLAYBACK_LOG_PARTS, ':').collect();
        if parts.len() != PLAYBACK_LOG_PARTS {
            continue;
        }
        let Ok(timestamp) = parts[0].parse::<i64>() else {
            continue;
        };
        let timestamp = local_timestamp_to_utc(timestamp);
        let Ok(elapsed_ms) = parts[1].parse::<i64>() else {
            continue;
        };
        let Ok(total_ms) = parts[2].parse::<i64>() else {
            continue;
        };
        entries.push(PlaybackEntry {
            timestamp,
            elapsed_ms,
            total_ms,
            path: parts[3].to_string(),
        });
    }
    Ok(entries)
}

fn tag_to_i32(tag: usize) -> i32 {
    i32::try_from(tag).expect("tag constants fit in i32")
}

fn local_timestamp_to_utc(timestamp: i64) -> i64 {
    let Some(utc_dt) = chrono::DateTime::<Utc>::from_timestamp(timestamp, 0) else {
        return timestamp;
    };
    let local_naive = utc_dt.naive_utc();
    match Local.from_local_datetime(&local_naive) {
        LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => {
            dt.with_timezone(&Utc).timestamp()
        }
        LocalResult::None => timestamp,
    }
}
