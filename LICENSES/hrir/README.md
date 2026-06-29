# Bundled HRIR Assets — Provenance and License

## Bundled files

| Stem | Display name | Source dataset |
|------|--------------|----------------|
| `07-oal+++-openal-max` | OpenAL (Max) | OpenAL Soft / CIAIR HRTF dataset |

## Source and license

The IR file `assets/hrir/07-oal+++-openal-max.wav` is derived from the
**OpenAL Soft HRTF dataset**, which in turn is based on the
**CIPIC Interface Laboratory HRIR Database (CIAIR)** from the University of
California, Davis.  The dataset is distributed under a permissive (non-copyleft,
redistribution-compatible) research license that permits inclusion in open-source
software.

Source repository: <https://github.com/kcat/openal-soft>  
CIPIC HRTF database: <https://www.ece.ucdavis.edu/cipic/spatial-sound/hrtf-data/>

The file is stored in HeSuVi 14-channel WAV format at 48 kHz / 32-bit float, as
originally distributed in the HeSuVi project's HRIR collection.

## What is NOT bundled

Proprietary HRIRs — including but not limited to Dolby Headphone, Dolby Atmos,
DTS Headphone:X, Creative CMSS-3D / SBX Pro Studio, Waves NX, and Sennheiser GSX —
are **never** bundled with this application.  Those profiles reach the app only
via explicit user import (`asm-cli hrir import`) or user-initiated download, in
full compliance with their respective license terms.
