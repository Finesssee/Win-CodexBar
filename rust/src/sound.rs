//! CodexBar の通知音再生。
//!
//! 通知別の独自 WAV、CodexBar 内蔵音、従来の Windows システム音を扱う。

#![allow(dead_code)]

use crate::settings::{NotificationSoundPaths, NotificationSoundTheme, Settings};
use serde::{Deserialize, Serialize};
use std::path::Path;

const MB_ICONERROR: u32 = 0x00000010;
const MB_ICONWARNING: u32 = 0x00000030;
const MB_ICONINFORMATION: u32 = 0x00000040;

const SAMPLE_RATE: u32 = 22_050;
const CHANNEL_COUNT: u16 = 1;
const BITS_PER_SAMPLE: u16 = 16;
const BYTES_PER_SAMPLE: u16 = BITS_PER_SAMPLE / 8;
const WAV_HEADER_SIZE: u32 = 44;
const RIFF_CHUNK_OVERHEAD: u32 = 36;
const PCM_FORMAT_CHUNK_SIZE: u32 = 16;
const PCM_FORMAT: u16 = 1;
const BASE_AMPLITUDE: f32 = 0.22;
const ATTACK_MS: u32 = 12;
const RELEASE_MS: u32 = 24;
const TWO_PI: f32 = std::f32::consts::PI * 2.0;

const NOTE_A3: f32 = 220.00;
const NOTE_C4: f32 = 261.63;
const NOTE_E4: f32 = 329.63;
const NOTE_G4: f32 = 392.00;
const NOTE_C5: f32 = 523.25;
const NOTE_E5: f32 = 659.25;
const NOTE_G5: f32 = 783.99;
const NOTE_A5: f32 = 880.00;
const NOTE_B5: f32 = 987.77;
const NOTE_C6: f32 = 1_046.50;

const SHORT_TONE_MS: u32 = 110;
const MEDIUM_TONE_MS: u32 = 160;
const LONG_TONE_MS: u32 = 260;
const SHORT_GAP_MS: u32 = 35;
const MEDIUM_GAP_MS: u32 = 55;
const NO_GAP_MS: u32 = 0;
const MILLISECONDS_PER_SECOND: u64 = 1_000;
const MIN_ENVELOPE_SAMPLES: usize = 1;

const TAG_SIZE: usize = 4;
const RIFF_TAG_OFFSET: usize = 0;
const WAVE_TAG_OFFSET: usize = 8;
const DATA_TAG_OFFSET: usize = 36;

/// 個別の音を割り当てられる通知イベント。
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

    fn windows_sound_type(self) -> u32 {
        match self {
            Self::PredictiveWarning | Self::HighUsage => MB_ICONWARNING,
            Self::CriticalUsage | Self::Exhausted | Self::StatusIssue | Self::SessionDepleted => {
                MB_ICONERROR
            }
            Self::SessionRestored => MB_ICONINFORMATION,
        }
    }

    fn built_in_pattern(self) -> &'static [Tone] {
        match self {
            Self::PredictiveWarning => &PREDICTIVE_WARNING_PATTERN,
            Self::HighUsage => &HIGH_USAGE_PATTERN,
            Self::CriticalUsage => &CRITICAL_USAGE_PATTERN,
            Self::Exhausted => &EXHAUSTED_PATTERN,
            Self::StatusIssue => &STATUS_ISSUE_PATTERN,
            Self::SessionDepleted => &SESSION_DEPLETED_PATTERN,
            Self::SessionRestored => &SESSION_RESTORED_PATTERN,
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
    #[error("Windows could not play the notification sound: {0}")]
    PlaybackFailed(String),
    #[error("could not start notification sound playback: {0}")]
    PlaybackThread(String),
    #[error("notification sound playback is not supported on this platform")]
    UnsupportedPlatform,
}

#[derive(Clone, Copy)]
struct Tone {
    frequency_hz: f32,
    duration_ms: u32,
    gap_after_ms: u32,
}

impl Tone {
    const fn new(frequency_hz: f32, duration_ms: u32, gap_after_ms: u32) -> Self {
        Self {
            frequency_hz,
            duration_ms,
            gap_after_ms,
        }
    }
}

const PREDICTIVE_WARNING_PATTERN: [Tone; 2] = [
    Tone::new(NOTE_G5, SHORT_TONE_MS, SHORT_GAP_MS),
    Tone::new(NOTE_B5, MEDIUM_TONE_MS, NO_GAP_MS),
];
const HIGH_USAGE_PATTERN: [Tone; 2] = [
    Tone::new(NOTE_A5, SHORT_TONE_MS, MEDIUM_GAP_MS),
    Tone::new(NOTE_A5, SHORT_TONE_MS, NO_GAP_MS),
];
const CRITICAL_USAGE_PATTERN: [Tone; 3] = [
    Tone::new(NOTE_C6, SHORT_TONE_MS, SHORT_GAP_MS),
    Tone::new(NOTE_G5, SHORT_TONE_MS, SHORT_GAP_MS),
    Tone::new(NOTE_C5, MEDIUM_TONE_MS, NO_GAP_MS),
];
const EXHAUSTED_PATTERN: [Tone; 2] = [
    Tone::new(NOTE_G4, MEDIUM_TONE_MS, MEDIUM_GAP_MS),
    Tone::new(NOTE_C4, LONG_TONE_MS, NO_GAP_MS),
];
const STATUS_ISSUE_PATTERN: [Tone; 3] = [
    Tone::new(NOTE_E5, SHORT_TONE_MS, SHORT_GAP_MS),
    Tone::new(NOTE_C5, SHORT_TONE_MS, SHORT_GAP_MS),
    Tone::new(NOTE_E5, MEDIUM_TONE_MS, NO_GAP_MS),
];
const SESSION_DEPLETED_PATTERN: [Tone; 3] = [
    Tone::new(NOTE_E4, MEDIUM_TONE_MS, SHORT_GAP_MS),
    Tone::new(NOTE_C4, MEDIUM_TONE_MS, SHORT_GAP_MS),
    Tone::new(NOTE_A3, LONG_TONE_MS, NO_GAP_MS),
];
const SESSION_RESTORED_PATTERN: [Tone; 3] = [
    Tone::new(NOTE_C5, SHORT_TONE_MS, SHORT_GAP_MS),
    Tone::new(NOTE_E5, SHORT_TONE_MS, SHORT_GAP_MS),
    Tone::new(NOTE_G5, LONG_TONE_MS, NO_GAP_MS),
];

/// 1つの通知イベントに設定された音を再生する。
pub fn play_alert(event: NotificationSoundEvent, settings: &Settings) -> Result<(), SoundError> {
    if !settings.sound_enabled {
        return Ok(());
    }

    if let Some(path) = event.custom_path(&settings.notification_sound_paths) {
        validate_custom_sound_path(path)?;
        return play_custom_wav(path);
    }

    match settings.notification_sound_theme {
        NotificationSoundTheme::Windows => play_windows_system_sound(event),
        NotificationSoundTheme::CodexBar => play_built_in_sound(event),
    }
}

/// 通知音として保存する前にファイルパスを検証する。
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
    Ok(())
}

/// 設定更新で追加または変更された独自通知音を検証する。
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
    use std::ffi::c_uint;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBeep(sound_type: c_uint) -> i32;
    }

    let result = unsafe { MessageBeep(event.windows_sound_type()) };
    if result == 0 {
        Err(SoundError::PlaybackFailed(format!("{event:?}")))
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
fn play_windows_system_sound(_event: NotificationSoundEvent) -> Result<(), SoundError> {
    Err(SoundError::UnsupportedPlatform)
}

#[cfg(target_os = "windows")]
fn play_custom_wav(path: &str) -> Result<(), SoundError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_NODEFAULT};
    use windows::core::PCWSTR;

    let wide_path: Vec<u16> = Path::new(path)
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let played = unsafe {
        PlaySoundW(
            PCWSTR(wide_path.as_ptr()),
            HMODULE::default(),
            SND_ASYNC | SND_FILENAME | SND_NODEFAULT,
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
    let wav = synthesize_wav(event.built_in_pattern());
    std::thread::Builder::new()
        .name("codexbar-notification-sound".to_string())
        .spawn(move || {
            use windows::Win32::Foundation::HMODULE;
            use windows::Win32::Media::Audio::{PlaySoundA, SND_MEMORY, SND_NODEFAULT};
            use windows::core::PCSTR;

            let played = unsafe {
                PlaySoundA(
                    PCSTR(wav.as_ptr()),
                    HMODULE::default(),
                    SND_MEMORY | SND_NODEFAULT,
                )
            };
            if !played.as_bool() {
                tracing::warn!(
                    ?event,
                    "CodexBar built-in notification sound failed to play"
                );
            }
        })
        .map_err(|error| SoundError::PlaybackThread(error.to_string()))?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn play_built_in_sound(_event: NotificationSoundEvent) -> Result<(), SoundError> {
    Err(SoundError::UnsupportedPlatform)
}

fn synthesize_wav(pattern: &[Tone]) -> Vec<u8> {
    let sample_count: usize = pattern
        .iter()
        .map(|tone| samples_for_ms(tone.duration_ms + tone.gap_after_ms))
        .sum();
    let data_size = sample_count * usize::from(BYTES_PER_SAMPLE);
    let mut wav = Vec::with_capacity(WAV_HEADER_SIZE as usize + data_size);

    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(RIFF_CHUNK_OVERHEAD + data_size as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&PCM_FORMAT_CHUNK_SIZE.to_le_bytes());
    wav.extend_from_slice(&PCM_FORMAT.to_le_bytes());
    wav.extend_from_slice(&CHANNEL_COUNT.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    let byte_rate = SAMPLE_RATE * u32::from(CHANNEL_COUNT) * u32::from(BYTES_PER_SAMPLE);
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    let block_align = CHANNEL_COUNT * BYTES_PER_SAMPLE;
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_size as u32).to_le_bytes());

    for tone in pattern {
        append_tone(&mut wav, *tone);
        append_silence(&mut wav, tone.gap_after_ms);
    }
    wav
}

fn samples_for_ms(duration_ms: u32) -> usize {
    (u64::from(SAMPLE_RATE) * u64::from(duration_ms) / MILLISECONDS_PER_SECOND) as usize
}

fn append_tone(wav: &mut Vec<u8>, tone: Tone) {
    let tone_samples = samples_for_ms(tone.duration_ms);
    let attack_samples = samples_for_ms(ATTACK_MS).max(MIN_ENVELOPE_SAMPLES);
    let release_samples = samples_for_ms(RELEASE_MS).max(MIN_ENVELOPE_SAMPLES);
    for sample_index in 0..tone_samples {
        let attack = sample_index as f32 / attack_samples as f32;
        let release = (tone_samples - sample_index) as f32 / release_samples as f32;
        let envelope = attack.min(release).min(1.0);
        let time = sample_index as f32 / SAMPLE_RATE as f32;
        let sample =
            (TWO_PI * tone.frequency_hz * time).sin() * envelope * BASE_AMPLITUDE * i16::MAX as f32;
        wav.extend_from_slice(&(sample as i16).to_le_bytes());
    }
}

fn append_silence(wav: &mut Vec<u8>, duration_ms: u32) {
    for _ in 0..samples_for_ms(duration_ms) {
        wav.extend_from_slice(&0_i16.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            NotificationSoundEvent::PredictiveWarning.windows_sound_type(),
            MB_ICONWARNING
        );
        assert_eq!(
            NotificationSoundEvent::HighUsage.windows_sound_type(),
            MB_ICONWARNING
        );
        for event in [
            NotificationSoundEvent::CriticalUsage,
            NotificationSoundEvent::Exhausted,
            NotificationSoundEvent::StatusIssue,
            NotificationSoundEvent::SessionDepleted,
        ] {
            assert_eq!(event.windows_sound_type(), MB_ICONERROR);
        }
        assert_eq!(
            NotificationSoundEvent::SessionRestored.windows_sound_type(),
            MB_ICONINFORMATION
        );
    }

    #[test]
    fn built_in_events_have_distinct_valid_wav_data() {
        let mut wav_data = Vec::new();
        for event in ALL_EVENTS {
            let wav = synthesize_wav(event.built_in_pattern());
            assert_eq!(&wav[RIFF_TAG_OFFSET..RIFF_TAG_OFFSET + TAG_SIZE], b"RIFF");
            assert_eq!(&wav[WAVE_TAG_OFFSET..WAVE_TAG_OFFSET + TAG_SIZE], b"WAVE");
            assert_eq!(&wav[DATA_TAG_OFFSET..DATA_TAG_OFFSET + TAG_SIZE], b"data");
            assert!(wav.len() > WAV_HEADER_SIZE as usize);
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
