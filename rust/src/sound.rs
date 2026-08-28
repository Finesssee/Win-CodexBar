//! Notification sound playback for CodexBar.
//!
//! Handles per-event custom WAV files, built-in CodexBar sounds, and Windows system sounds.

#![allow(
    dead_code,
    reason = "sound playback types reserved for future alert audio integration"
)]

use crate::settings::{NotificationSoundPaths, NotificationSoundTheme, Settings};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[cfg(target_os = "windows")]
use std::sync::{OnceLock, mpsc};

const MINIMUM_WAV_SIZE: usize = 44;
const TAG_SIZE: usize = 4;
const RIFF_TAG_OFFSET: usize = 0;
const RIFF_SIZE_OFFSET: usize = 4;
const RIFF_SIZE_BASE: usize = 8;
const WAVE_TAG_OFFSET: usize = 8;
const RIFF_HEADER_SIZE: usize = 12;
const CHUNK_HEADER_SIZE: usize = 8;
const CHUNK_SIZE_OFFSET: usize = 4;
const MINIMUM_FORMAT_CHUNK_SIZE: usize = 16;

const PREDICTIVE_WARNING_WAV: &[u8] = include_bytes!("../assets/sounds/predictive-warning.wav");
const HIGH_USAGE_WAV: &[u8] = include_bytes!("../assets/sounds/high-usage.wav");
const CRITICAL_USAGE_WAV: &[u8] = include_bytes!("../assets/sounds/critical-usage.wav");
const EXHAUSTED_WAV: &[u8] = include_bytes!("../assets/sounds/exhausted.wav");
const STATUS_ISSUE_WAV: &[u8] = include_bytes!("../assets/sounds/status-issue.wav");
const SESSION_DEPLETED_WAV: &[u8] = include_bytes!("../assets/sounds/session-depleted.wav");
const SESSION_RESTORED_WAV: &[u8] = include_bytes!("../assets/sounds/session-restored.wav");

/// Notification events that can have individual sounds assigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationSoundEvent {
    PredictiveWarning,
    HighUsage,
    CriticalUsage,
    Exhausted,
    StatusIssue,
    SessionDepleted,
    SessionRestored,
}

impl NotificationSoundEvent {
    fn custom_path(self, paths: &NotificationSoundPaths) -> Option<&str> {
        match self {
            Self::PredictiveWarning => paths.predictive_warning.as_deref(),
            Self::HighUsage => paths.high_usage.as_deref(),
            Self::CriticalUsage => paths.critical_usage.as_deref(),
            Self::Exhausted => paths.exhausted.as_deref(),
            Self::StatusIssue => paths.status_issue.as_deref(),
            Self::SessionDepleted => paths.session_depleted.as_deref(),
            Self::SessionRestored => paths.session_restored.as_deref(),
        }
    }

    fn windows_sound_alias(self) -> &'static str {
        match self {
            Self::PredictiveWarning | Self::HighUsage => "SystemExclamation",
            Self::CriticalUsage | Self::Exhausted | Self::StatusIssue | Self::SessionDepleted => {
                "SystemHand"
            }
            Self::SessionRestored => "SystemAsterisk",
        }
    }

    fn built_in_wav(self) -> &'static [u8] {
        match self {
            Self::PredictiveWarning => PREDICTIVE_WARNING_WAV,
            Self::HighUsage => HIGH_USAGE_WAV,
            Self::CriticalUsage => CRITICAL_USAGE_WAV,
            Self::Exhausted => EXHAUSTED_WAV,
            Self::StatusIssue => STATUS_ISSUE_WAV,
            Self::SessionDepleted => SESSION_DEPLETED_WAV,
            Self::SessionRestored => SESSION_RESTORED_WAV,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SoundError {
    #[error("notification sound path must be absolute: {0}")]
    RelativePath(String),
    #[error("notification sound file does not exist: {0}")]
    MissingFile(String),
    #[error("notification sound must be a WAV file: {0}")]
    UnsupportedFormat(String),
    #[error("could not read notification sound file {path}: {source}")]
    ReadFailed {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("notification sound is not a valid WAV file ({reason}): {path}")]
    InvalidWav { path: String, reason: &'static str },
    #[error("Windows could not play the notification sound: {0}")]
    PlaybackFailed(String),
    #[error("could not start notification sound playback: {0}")]
    PlaybackThread(String),
    #[error("notification sound playback is not supported on this platform")]
    UnsupportedPlatform,
}

/// Play the sound configured for one notification event.
pub fn play_alert(event: NotificationSoundEvent, settings: &Settings) -> Result<(), SoundError> {
    if !settings.sound_enabled {
        return Ok(());
    }

    if let Some(path) = event.custom_path(&settings.notification_sound_paths) {
        return play_custom_wav(path);
    }

    match settings.notification_sound_theme {
        NotificationSoundTheme::Windows => play_windows_system_sound(event),
        NotificationSoundTheme::CodexBar => play_built_in_sound(event),
    }
}

/// Validate a file path before saving it as a notification sound.
pub fn validate_custom_sound_path(path: &str) -> Result<(), SoundError> {
    let candidate = Path::new(path);
    if !candidate.is_absolute() {
        return Err(SoundError::RelativePath(path.to_string()));
    }
    if !candidate.is_file() {
        return Err(SoundError::MissingFile(path.to_string()));
    }
    let is_wav = candidate
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"));
    if !is_wav {
        return Err(SoundError::UnsupportedFormat(path.to_string()));
    }
    let wav = std::fs::read(candidate).map_err(|source| SoundError::ReadFailed {
        path: path.to_string(),
        source,
    })?;
    validate_wav_bytes(&wav).map_err(|reason| SoundError::InvalidWav {
        path: path.to_string(),
        reason,
    })?;
    Ok(())
}

fn validate_wav_bytes(wav: &[u8]) -> Result<(), &'static str> {
    if wav.len() < RIFF_HEADER_SIZE {
        return Err("file is shorter than the RIFF header");
    }
    if &wav[RIFF_TAG_OFFSET..RIFF_TAG_OFFSET + TAG_SIZE] != b"RIFF"
        || &wav[WAVE_TAG_OFFSET..WAVE_TAG_OFFSET + TAG_SIZE] != b"WAVE"
    {
        return Err("RIFF/WAVE signature is missing");
    }

    let riff_size = u32::from_le_bytes(
        wav[RIFF_SIZE_OFFSET..RIFF_SIZE_OFFSET + TAG_SIZE]
            .try_into()
            .expect("RIFF size field"),
    ) as usize;
    let riff_end = RIFF_SIZE_BASE
        .checked_add(riff_size)
        .ok_or("RIFF size overflows")?;
    if riff_end < RIFF_HEADER_SIZE || riff_end > wav.len() {
        return Err("RIFF size exceeds the file bounds");
    }

    let mut offset = RIFF_HEADER_SIZE;
    let mut has_format = false;
    let mut has_audio_data = false;
    while offset + CHUNK_HEADER_SIZE <= riff_end {
        let chunk_size = u32::from_le_bytes(
            wav[offset + CHUNK_SIZE_OFFSET..offset + CHUNK_HEADER_SIZE]
                .try_into()
                .expect("WAV chunk size field"),
        ) as usize;
        let data_start = offset + CHUNK_HEADER_SIZE;
        let data_end = data_start
            .checked_add(chunk_size)
            .ok_or("WAV chunk size overflows")?;
        if data_end > riff_end {
            return Err("WAV chunk exceeds the RIFF bounds");
        }

        match &wav[offset..offset + TAG_SIZE] {
            b"fmt " if chunk_size >= MINIMUM_FORMAT_CHUNK_SIZE => has_format = true,
            b"fmt " => return Err("format chunk is too short"),
            b"data" if chunk_size > 0 => has_audio_data = true,
            _ => {}
        }

        let padding = chunk_size % 2;
        offset = data_end
            .checked_add(padding)
            .ok_or("WAV chunk padding overflows")?;
        if offset > riff_end {
            return Err("WAV chunk padding exceeds the RIFF bounds");
        }
    }

    if !has_format {
        return Err("format chunk is missing");
    }
    if !has_audio_data {
        return Err("audio data chunk is missing or empty");
    }
    Ok(())
}

/// Validate custom notification sounds added or changed by a settings update.
pub fn validate_custom_sound_path_updates(
    current: &NotificationSoundPaths,
    updated: &NotificationSoundPaths,
) -> Result<(), SoundError> {
    let path_pairs = [
        (
            current.predictive_warning.as_deref(),
            updated.predictive_warning.as_deref(),
        ),
        (current.high_usage.as_deref(), updated.high_usage.as_deref()),
        (
            current.critical_usage.as_deref(),
            updated.critical_usage.as_deref(),
        ),
        (current.exhausted.as_deref(), updated.exhausted.as_deref()),
        (
            current.status_issue.as_deref(),
            updated.status_issue.as_deref(),
        ),
        (
            current.session_depleted.as_deref(),
            updated.session_depleted.as_deref(),
        ),
        (
            current.session_restored.as_deref(),
            updated.session_restored.as_deref(),
        ),
    ];

    for (current_path, updated_path) in path_pairs {
        if updated_path != current_path
            && let Some(path) = updated_path
        {
            validate_custom_sound_path(path)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn play_windows_system_sound(event: NotificationSoundEvent) -> Result<(), SoundError> {
    enqueue_playback(PlaybackRequest::WindowsSystem(event))
}

#[cfg(target_os = "windows")]
fn perform_windows_system_sound(event: NotificationSoundEvent) -> Result<(), SoundError> {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_NODEFAULT};
    use windows::core::PCWSTR;

    let wide_alias: Vec<u16> = event
        .windows_sound_alias()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide_alias` is a null-terminated UTF-16 buffer owned by this
    // scope that outlives the synchronous call. `hmod` is the null handle
    // (required when using SND_ALIAS with no module resource). The flags play
    // the registered system alias synchronously with no default fallback.
    let played = unsafe {
        PlaySoundW(
            PCWSTR(wide_alias.as_ptr()),
            HMODULE::default(),
            SND_ALIAS | SND_NODEFAULT,
        )
    };
    if played.as_bool() {
        Ok(())
    } else {
        Err(SoundError::PlaybackFailed(format!("{event:?}")))
    }
}

#[cfg(not(target_os = "windows"))]
fn play_windows_system_sound(_event: NotificationSoundEvent) -> Result<(), SoundError> {
    Err(SoundError::UnsupportedPlatform)
}

#[cfg(target_os = "windows")]
fn play_custom_wav(path: &str) -> Result<(), SoundError> {
    enqueue_playback(PlaybackRequest::CustomWav(path.to_string()))
}

#[cfg(target_os = "windows")]
fn perform_custom_wav(path: &str) -> Result<(), SoundError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_FILENAME, SND_NODEFAULT};
    use windows::core::PCWSTR;

    let wide_path: Vec<u16> = Path::new(path)
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide_path` is a null-terminated UTF-16 buffer owned by this
    // scope that outlives the synchronous call. `hmod` is the null handle
    // (required for SND_FILENAME, which names a file rather than a module
    // resource). Playback is synchronous, so the buffer stays valid through
    // the whole call.
    let played = unsafe {
        PlaySoundW(
            PCWSTR(wide_path.as_ptr()),
            HMODULE::default(),
            SND_FILENAME | SND_NODEFAULT,
        )
    };
    if played.as_bool() {
        Ok(())
    } else {
        Err(SoundError::PlaybackFailed(path.to_string()))
    }
}

#[cfg(not(target_os = "windows"))]
fn play_custom_wav(_path: &str) -> Result<(), SoundError> {
    Err(SoundError::UnsupportedPlatform)
}

#[cfg(target_os = "windows")]
fn play_built_in_sound(event: NotificationSoundEvent) -> Result<(), SoundError> {
    enqueue_playback(PlaybackRequest::BuiltIn(event))
}

#[cfg(target_os = "windows")]
fn perform_built_in_sound(event: NotificationSoundEvent) -> Result<(), SoundError> {
    let wav = event.built_in_wav();
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Media::Audio::{PlaySoundA, SND_MEMORY, SND_NODEFAULT};
    use windows::core::PCSTR;

    // SAFETY: `wav` is static bytes embedded via `include_bytes!`, so its
    // pointer is valid for the program's lifetime. `hmod` is the null handle
    // (required with SND_MEMORY, which reads the sound from the memory
    // buffer rather than a module resource). Playback is synchronous, so
    // the buffer stays valid through the whole call.
    let played = unsafe {
        PlaySoundA(
            PCSTR(wav.as_ptr()),
            HMODULE::default(),
            SND_MEMORY | SND_NODEFAULT,
        )
    };
    if played.as_bool() {
        Ok(())
    } else {
        Err(SoundError::PlaybackFailed(format!("{event:?}")))
    }
}

#[cfg(not(target_os = "windows"))]
fn play_built_in_sound(_event: NotificationSoundEvent) -> Result<(), SoundError> {
    Err(SoundError::UnsupportedPlatform)
}

#[cfg(target_os = "windows")]
enum PlaybackRequest {
    WindowsSystem(NotificationSoundEvent),
    BuiltIn(NotificationSoundEvent),
    CustomWav(String),
}

#[cfg(target_os = "windows")]
static PLAYBACK_QUEUE: OnceLock<Result<mpsc::Sender<PlaybackRequest>, String>> = OnceLock::new();

#[cfg(target_os = "windows")]
fn enqueue_playback(request: PlaybackRequest) -> Result<(), SoundError> {
    let sender = PLAYBACK_QUEUE.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("codexbar-notification-sound".to_string())
            .spawn(move || playback_worker(receiver))
            .map(|_| sender)
            .map_err(|error| error.to_string())
    });
    match sender {
        Ok(sender) => sender
            .send(request)
            .map_err(|error| SoundError::PlaybackThread(error.to_string())),
        Err(error) => Err(SoundError::PlaybackThread(error.clone())),
    }
}

#[cfg(target_os = "windows")]
fn playback_worker(receiver: mpsc::Receiver<PlaybackRequest>) {
    for request in receiver {
        let result = match request {
            PlaybackRequest::WindowsSystem(event) => perform_windows_system_sound(event),
            PlaybackRequest::BuiltIn(event) => perform_built_in_sound(event),
            PlaybackRequest::CustomWav(path) => perform_custom_wav(&path),
        };
        if let Err(error) = result {
            tracing::warn!(%error, "notification sound failed to play");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_chunk<'a>(wav: &'a [u8], tag: &[u8; TAG_SIZE]) -> Option<&'a [u8]> {
        let mut offset = RIFF_HEADER_SIZE;
        while offset + CHUNK_HEADER_SIZE <= wav.len() {
            let chunk_size = u32::from_le_bytes(
                wav[offset + CHUNK_SIZE_OFFSET..offset + CHUNK_HEADER_SIZE]
                    .try_into()
                    .expect("WAV chunk size"),
            ) as usize;
            let data_start = offset + CHUNK_HEADER_SIZE;
            let data_end = data_start.saturating_add(chunk_size);
            if &wav[offset..offset + TAG_SIZE] == tag && data_end <= wav.len() {
                return Some(&wav[data_start..data_end]);
            }
            let padding = chunk_size % 2;
            offset = data_end.saturating_add(padding);
        }
        None
    }

    const ALL_EVENTS: [NotificationSoundEvent; 7] = [
        NotificationSoundEvent::PredictiveWarning,
        NotificationSoundEvent::HighUsage,
        NotificationSoundEvent::CriticalUsage,
        NotificationSoundEvent::Exhausted,
        NotificationSoundEvent::StatusIssue,
        NotificationSoundEvent::SessionDepleted,
        NotificationSoundEvent::SessionRestored,
    ];

    #[test]
    fn windows_defaults_preserve_existing_alert_mapping() {
        assert_eq!(
            NotificationSoundEvent::PredictiveWarning.windows_sound_alias(),
            "SystemExclamation"
        );
        assert_eq!(
            NotificationSoundEvent::HighUsage.windows_sound_alias(),
            "SystemExclamation"
        );
        for event in [
            NotificationSoundEvent::CriticalUsage,
            NotificationSoundEvent::Exhausted,
            NotificationSoundEvent::StatusIssue,
            NotificationSoundEvent::SessionDepleted,
        ] {
            assert_eq!(event.windows_sound_alias(), "SystemHand");
        }
        assert_eq!(
            NotificationSoundEvent::SessionRestored.windows_sound_alias(),
            "SystemAsterisk"
        );
    }

    #[test]
    fn built_in_events_have_distinct_valid_wav_data() {
        let mut wav_data = Vec::new();
        for event in ALL_EVENTS {
            let wav = event.built_in_wav();
            assert_eq!(&wav[RIFF_TAG_OFFSET..RIFF_TAG_OFFSET + TAG_SIZE], b"RIFF");
            assert_eq!(&wav[WAVE_TAG_OFFSET..WAVE_TAG_OFFSET + TAG_SIZE], b"WAVE");
            assert!(validate_wav_bytes(wav).is_ok());
            assert!(wav.len() > MINIMUM_WAV_SIZE);
            let format = find_chunk(wav, b"fmt ").expect("format chunk");
            assert_eq!(
                u16::from_le_bytes(format[0..2].try_into().expect("audio format")),
                1
            );
            assert_eq!(
                u16::from_le_bytes(format[2..4].try_into().expect("channel count")),
                2
            );
            assert_eq!(
                u32::from_le_bytes(format[4..8].try_into().expect("sample rate")),
                48_000
            );
            assert_eq!(
                u16::from_le_bytes(format[14..16].try_into().expect("bit depth")),
                16
            );
            wav_data.push(wav);
        }

        for first in 0..wav_data.len() {
            for second in (first + 1)..wav_data.len() {
                assert_ne!(wav_data[first], wav_data[second]);
            }
        }
    }

    #[test]
    fn each_event_reads_only_its_custom_path() {
        let paths = NotificationSoundPaths {
            predictive_warning: Some("predictive.wav".to_string()),
            high_usage: Some("high.wav".to_string()),
            critical_usage: Some("critical.wav".to_string()),
            exhausted: Some("exhausted.wav".to_string()),
            status_issue: Some("status.wav".to_string()),
            session_depleted: Some("depleted.wav".to_string()),
            session_restored: Some("restored.wav".to_string()),
        };
        let expected = [
            "predictive.wav",
            "high.wav",
            "critical.wav",
            "exhausted.wav",
            "status.wav",
            "depleted.wav",
            "restored.wav",
        ];
        for (event, expected_path) in ALL_EVENTS.into_iter().zip(expected) {
            assert_eq!(event.custom_path(&paths), Some(expected_path));
        }
    }

    #[test]
    fn custom_sound_validation_rejects_relative_and_non_wav_paths() {
        assert!(matches!(
            validate_custom_sound_path("relative.wav"),
            Err(SoundError::RelativePath(_))
        ));

        let temp = tempfile::tempdir().expect("create temp directory");
        let mp3 = temp.path().join("sound.mp3");
        std::fs::write(&mp3, b"not audio").expect("write test file");
        assert!(matches!(
            validate_custom_sound_path(mp3.to_str().expect("UTF-8 test path")),
            Err(SoundError::UnsupportedFormat(_))
        ));

        let fake_wav = temp.path().join("renamed.wav");
        std::fs::write(&fake_wav, b"not audio").expect("write test file");
        assert!(matches!(
            validate_custom_sound_path(fake_wav.to_str().expect("UTF-8 test path")),
            Err(SoundError::InvalidWav { .. })
        ));
    }

    #[test]
    fn path_update_validation_allows_clearing_one_of_multiple_missing_files() {
        let current = NotificationSoundPaths {
            high_usage: Some(r"C:\missing\high.wav".to_string()),
            critical_usage: Some(r"C:\missing\critical.wav".to_string()),
            ..NotificationSoundPaths::default()
        };
        let updated = NotificationSoundPaths {
            high_usage: None,
            ..current.clone()
        };

        assert!(validate_custom_sound_path_updates(&current, &updated).is_ok());
    }
}
