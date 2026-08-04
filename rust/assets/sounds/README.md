# CodexBar notification sounds

These original notification sounds were created for CodexBar with deterministic
procedural DSP. They contain no sampled or third-party audio and are distributed
under the repository's MIT License.

All files are 48 kHz, stereo, 16-bit PCM WAV files. They were checked for silence,
normalized with EBU R128 loudness measurement, and verified below -1.5 dBTP true
peak. Critical usage is intentionally louder than ordinary notifications, while
the exhausted cue is compensated for its lower-frequency content.

| File | Intended meaning |
| --- | --- |
| `predictive-warning.wav` | Clock-like onset and two rising tones for an early forecast |
| `high-usage.wav` | Two equal meter-warning pulses |
| `critical-usage.wav` | Four rapid alternating alarm pulses and a firm final pulse |
| `exhausted.wav` | A restrained cutoff followed by a low terminal chord |
| `status-issue.wav` | A data glitch and dissonant interval for a provider fault |
| `session-depleted.wav` | A low-battery pattern for temporary session depletion |
| `session-restored.wav` | A bright ascending completion arpeggio |
