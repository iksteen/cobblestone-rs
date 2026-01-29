use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;

use crate::config::{Account, ServiceKeys};
use crate::scrobble::ScrobbleTrack;

#[derive(Debug, Clone, Copy)]
pub enum Service {
    LastFm,
    LibreFm,
}

impl Service {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "lastfm" => Ok(Service::LastFm),
            "librefm" => Ok(Service::LibreFm),
            _ => bail!("Unsupported service: {value}"),
        }
    }

    pub fn base_url(self) -> &'static str {
        match self {
            Service::LastFm => "https://ws.audioscrobbler.com/2.0/",
            Service::LibreFm => "https://libre.fm/2.0/",
        }
    }
}

pub struct ScrobbleClient {
    service: Service,
    api_key: String,
    api_secret: String,
    session_key: String,
    http: Client,
    debug_response: bool,
}

impl ScrobbleClient {
    pub fn new(
        service: Service,
        keys: &ServiceKeys,
        account: &Account,
        debug_response: bool,
    ) -> Result<Self> {
        let http = Client::builder()
            .build()
            .context("Failed building HTTP client")?;
        let session_key =
            fetch_mobile_session(&http, service, &keys.api_key, &keys.api_secret, account)?;
        Ok(Self {
            service,
            api_key: keys.api_key.clone(),
            api_secret: keys.api_secret.clone(),
            session_key,
            http,
            debug_response,
        })
    }

    pub fn scrobble_tracks(&self, tracks: &[ScrobbleTrack]) -> Vec<String> {
        let mut errors = Vec::new();
        for track in tracks {
            if let Err(err) = self.scrobble_track(track) {
                errors.push(format!("{} - {}: {}", track.artist, track.title, err));
            }
        }
        errors
    }

    fn scrobble_track(&self, track: &ScrobbleTrack) -> Result<()> {
        let mut params = vec![
            ("method".to_string(), "track.scrobble".to_string()),
            ("artist".to_string(), track.artist.clone()),
            ("track".to_string(), track.title.clone()),
            ("timestamp".to_string(), track.timestamp.to_string()),
            ("api_key".to_string(), self.api_key.clone()),
            ("sk".to_string(), self.session_key.clone()),
        ];
        if let Some(album) = &track.album {
            params.push(("album".to_string(), album.clone()));
        }
        if track.duration > 0 {
            params.push(("duration".to_string(), track.duration.to_string()));
        }
        let api_sig = sign_params(&params, &self.api_secret);
        params.push(("api_sig".to_string(), api_sig));
        params.push(("format".to_string(), "json".to_string()));
        let response = self
            .http
            .post(self.service.base_url())
            .form(&params)
            .send()
            .context("Failed sending scrobble request")?;
        let text = response
            .text()
            .context("Failed reading scrobble response")?;
        if self.debug_response {
            eprintln!(
                "Scrobble response from {}: {}",
                self.service.base_url(),
                text
            );
        }
        check_api_error(&text)?;
        check_scrobble_result(&text)?;
        Ok(())
    }
}

fn fetch_mobile_session(
    http: &Client,
    service: Service,
    api_key: &str,
    api_secret: &str,
    account: &Account,
) -> Result<String> {
    let auth_token = format!(
        "{:x}",
        md5::compute(format!("{}{}", account.username, account.password_md5))
    );
    let mut params = vec![
        ("method".to_string(), "auth.getMobileSession".to_string()),
        ("username".to_string(), account.username.clone()),
        ("authToken".to_string(), auth_token),
        ("api_key".to_string(), api_key.to_string()),
    ];
    let api_sig = sign_params(&params, api_secret);
    params.push(("api_sig".to_string(), api_sig));
    params.push(("format".to_string(), "json".to_string()));
    let response = http
        .post(service.base_url())
        .form(&params)
        .send()
        .context("Failed requesting mobile session")?;
    let text = response.text().context("Failed reading session response")?;
    check_api_error(&text)?;
    let json: Value = serde_json::from_str(&text).context("Failed parsing session response")?;
    let key = json
        .get("session")
        .and_then(|session| session.get("key"))
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("Missing session key in response"))?;
    Ok(key)
}

fn sign_params(params: &[(String, String)], secret: &str) -> String {
    let mut sorted = params.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut signature = String::new();
    for (key, value) in sorted {
        signature.push_str(&key);
        signature.push_str(&value);
    }
    signature.push_str(secret);
    format!("{:x}", md5::compute(signature))
}

fn check_api_error(payload: &str) -> Result<()> {
    let json: Value = serde_json::from_str(payload).context("Failed parsing API response")?;
    if let Some(error) = json.get("error") {
        let message = json
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("API error");
        bail!("API error {error}: {message}");
    }
    Ok(())
}

fn check_scrobble_result(payload: &str) -> Result<()> {
    if let Ok(parsed) = serde_json::from_str::<ScrobbleResponse>(payload) {
        return check_scrobble_from_struct(&parsed);
    }
    if let Ok(parsed) = serde_json::from_str::<Value>(payload) {
        return check_scrobble_from_value(&parsed);
    }
    Ok(())
}

fn check_scrobble_from_struct(parsed: &ScrobbleResponse) -> Result<()> {
    let Some(scrobbles) = parsed.scrobbles.as_ref() else {
        return Ok(());
    };
    let accepted = scrobbles.attr.as_ref().map_or(0, |attr| attr.accepted);
    let ignored = scrobbles.attr.as_ref().map_or(0, |attr| attr.ignored);
    if accepted > 0 && ignored == 0 {
        return Ok(());
    }
    let ignored = scrobbles
        .scrobble
        .as_ref()
        .and_then(ScrobbleEntries::first_ignored_message);
    let (ignored_code, ignored_message) = match ignored {
        Some(IgnoredMessageField::Object(message)) => {
            let code = message
                .code
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let text = message
                .text
                .clone()
                .unwrap_or_else(|| "Scrobble rejected".to_string());
            (code, text)
        }
        Some(IgnoredMessageField::Text(message)) => ("unknown".to_string(), message.clone()),
        Some(IgnoredMessageField::Number(code)) => ("unknown".to_string(), code.to_string()),
        None => ("unknown".to_string(), "Scrobble rejected".to_string()),
    };
    if ignored_code == "91" {
        return Ok(());
    }
    bail!("Scrobble rejected (code {ignored_code}): {ignored_message}");
}

fn check_scrobble_from_value(parsed: &Value) -> Result<()> {
    let Some(scrobbles) = parsed.get("scrobbles") else {
        return Ok(());
    };
    let attr = scrobbles.get("@attr");
    let accepted = attr
        .and_then(|value| value.get("accepted"))
        .and_then(parse_u32_value)
        .unwrap_or(0);
    let ignored = attr
        .and_then(|value| value.get("ignored"))
        .and_then(parse_u32_value)
        .unwrap_or(0);
    if accepted > 0 && ignored == 0 {
        return Ok(());
    }
    let ignored_message = scrobbles
        .get("scrobble")
        .and_then(first_scrobble_value)
        .and_then(|value| value.get("ignoredMessage"))
        .map_or_else(
            || ("unknown".to_string(), "Scrobble rejected".to_string()),
            ignored_message_from_value,
        );
    let (code, message) = ignored_message;
    bail!("Scrobble rejected (code {code}): {message}");
}

fn parse_u32_value(value: &Value) -> Option<u32> {
    if let Some(value) = value.as_u64() {
        return u32::try_from(value).ok();
    }
    value.as_str().and_then(|raw| raw.parse::<u32>().ok())
}

fn first_scrobble_value(value: &Value) -> Option<&Value> {
    if let Some(array) = value.as_array() {
        return array.first();
    }
    if value.is_object() {
        return Some(value);
    }
    None
}

fn ignored_message_from_value(value: &Value) -> (String, String) {
    if let Some(text) = value.as_str() {
        return ("unknown".to_string(), text.to_string());
    }
    if let Some(number) = value.as_u64() {
        return ("unknown".to_string(), number.to_string());
    }
    let code = value
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let text = value
        .get("#text")
        .and_then(Value::as_str)
        .unwrap_or("Scrobble rejected");
    (code.to_string(), text.to_string())
}

#[derive(Debug, Deserialize)]
struct ScrobbleResponse {
    #[serde(default)]
    scrobbles: Option<Scrobbles>,
}

#[derive(Debug, Deserialize)]
struct Scrobbles {
    #[serde(rename = "@attr")]
    #[serde(default)]
    attr: Option<ScrobbleAttr>,
    #[serde(default)]
    scrobble: Option<ScrobbleEntries>,
}

#[derive(Debug, Deserialize)]
struct ScrobbleAttr {
    #[serde(deserialize_with = "deserialize_u32_string_or_number")]
    accepted: u32,
    #[serde(deserialize_with = "deserialize_u32_string_or_number")]
    ignored: u32,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ScrobbleEntries {
    One(ScrobbleEntry),
    Many(Vec<ScrobbleEntry>),
}

impl ScrobbleEntries {
    fn first_ignored_message(&self) -> Option<&IgnoredMessageField> {
        match self {
            ScrobbleEntries::One(entry) => entry.ignored_message.as_ref(),
            ScrobbleEntries::Many(entries) => entries
                .first()
                .and_then(|entry| entry.ignored_message.as_ref()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ScrobbleEntry {
    #[serde(rename = "ignoredMessage")]
    #[serde(default)]
    ignored_message: Option<IgnoredMessageField>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum IgnoredMessageField {
    Object(IgnoredMessage),
    Text(String),
    Number(u32),
}

#[derive(Debug, Deserialize)]
struct IgnoredMessage {
    #[serde(rename = "#text")]
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

fn deserialize_u32_string_or_number<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrU32 {
        String(String),
        Number(u32),
    }
    match StringOrU32::deserialize(deserializer)? {
        StringOrU32::String(value) => value.parse::<u32>().map_err(serde::de::Error::custom),
        StringOrU32::Number(value) => Ok(value),
    }
}
