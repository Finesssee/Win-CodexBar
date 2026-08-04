# CodexBar notification sounds

These original notification sounds were created for CodexBar with deterministic
procedural DSP. They contain no sampled or third-party audio and are distributed
under the repository's MIT License.

The source synthesis used a fixed random seed of 7291 with NumPy and SciPy.
Otoforge performed the final EBU R128 loudness normalization and true-peak
validation. All files are 48 kHz, stereo, 16-bit PCM WAV files. Critical usage is
intentionally louder than ordinary notifications, while the exhausted cue is
compensated for its lower-frequency content.

| File | Intended meaning | Duration | Integrated loudness | SHA-256 |
| --- | --- | ---: | ---: | --- |
| `predictive-warning.wav` | Clock-like onset and two rising tones for an early forecast | 1.00 s | -18.0 LUFS | `10301f4def1113f26d8b1366d64f62b6ee5ed33ed6d001e6e32e3a8667e1ec91` |
| `high-usage.wav` | Two equal meter-warning pulses | 0.82 s | -18.0 LUFS | `6629ec0ce14e531b70b900af78495c3c071044fea2bbf1ec09aca31b86d7ba29` |
| `critical-usage.wav` | Four rapid alternating alarm pulses and a firm final pulse | 0.84 s | -15.0 LUFS | `75aa34ac08ccb2d430cd778452e4f376f8953f134b3200fed4793ad367af3469` |
| `exhausted.wav` | A restrained cutoff followed by a low terminal chord | 1.10 s | -16.0 LUFS | `ebeb98efada1f8e61446811887b4679e950e3fa9c4e2498f2514ed3a5db3c2b3` |
| `status-issue.wav` | A data glitch and dissonant interval for a provider fault | 0.90 s | -18.0 LUFS | `6b9d4340a791d441d10977907e3d8010cd59a15d7a34369ab8a01852bbdf0c46` |
| `session-depleted.wav` | A low-battery pattern for temporary session depletion | 1.06 s | -18.0 LUFS | `10ef21a5c1f5dca9245669c640e5b49cd628abfca3dc1e1c836853ce26383ade` |
| `session-restored.wav` | A bright ascending completion arpeggio | 1.05 s | -18.0 LUFS | `ac653f8096ce87ad2405c1491f7ea20a62464ab2638575d2d6c5c6a68d3f04d6` |
