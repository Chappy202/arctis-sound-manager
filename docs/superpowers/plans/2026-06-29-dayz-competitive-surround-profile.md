# DayZ Competitive Surround Profile + Factory-Profile Catalog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (- [ ]) syntax.

**Goal:** Turn the one-off `dayz` factory profile into a research-backed "DayZ competitive positioning" preset (pinned GSX HRIR, blocksize 128, explicit post-convolution "DayZ Spatial" EQ) **and** restructure factory profiles into a data-driven catalog so future games are pure data, not new code paths. Rides on the already-live discrete-7.1 → HRIR pipeline; no new audio architecture.

**Architecture:** Everything game-specific becomes data in `factory_profiles::FactoryProfileSpec` (mirrors the existing HRIR `CATALOG` and `factory_eq_presets()` patterns); everything mechanical becomes a general capability any profile uses — two additive `SurroundConfig` fields (`output_eq`, `blocksize`), an explicit post-convolution EQ stage in `apply_surround`, a missing-pinned-HRIR fallback-with-prompt mechanism, and a data-driven Tauri/UI listing. Engine-first: Part A (Tasks 1–8) makes the DayZ profile fully work through the existing `profileCreateFromFactory("DayZ")` path with zero UI changes; Part B (Tasks 9–14) adds the data-driven listing, post-EQ controls, and the missing-HRIR prompt.

**Tech Stack:** Rust Cargo workspace (`arctis-config`, `arctis-audio`, `arctis-engine`, `arctis-client`, `src-tauri`) + Tauri v2 + Svelte 5 frontend; PipeWire filter-chain renderer; vitest for frontend pure logic.

## Global Constraints

- Rust workspace + Tauri v2 + Svelte 5; engine/config/audio are UI-agnostic (tauri only in `src-tauri`).
- Audio is **48 kHz only** — no resampling anywhere.
- **G7:** no `unwrap`/`expect`/`panic` on runtime paths; return typed errors (`EngineError`, `AudioError`, `ConfigError`). `unwrap` is allowed only inside `#[cfg(test)]`.
- **Additive `#[serde(default)]` schema only** — never break existing config TOML; old configs must load byte-for-byte to today's behavior.
- EQ bounds are **±12 dB**; band kinds are exactly `"peaking"` / `"lowshelf"` / `"highshelf"`.
- Frontend convention: **NO jsdom** — pure logic lives in `.ts` with vitest tests, `.svelte` files stay thin views.
- After each engine/config/preset change the daemon must be restarted for the GUI to see it (out of scope for tests; relevant only for manual verification).

---

# Part A — Engine + Audio core

> Tasks 1–8. On completion, `profileCreateFromFactory("DayZ")` (existing IPC) produces the full competitive profile: pinned GSX HRIR with safe fallback, blocksize 128, the "DayZ Spatial" post-convolution EQ on the binaural tail, game-channel surround, default sink = game — with **no UI changes required**.

---

### Task 1: Add `output_eq` + `blocksize` to `SurroundConfig` (schema)

**Model:** haiku

**Files**
- Modify `crates/config/src/schema.rs` — `SurroundConfig` struct (lines 342–368) and its `impl Default` (lines 370–381).
- Modify `crates/config/src/schema.rs` — add a round-trip test in the existing `#[cfg(test)] mod` (search for `mod tests` in the file; if `SurroundConfig` tests already exist, add beside them).

**Interfaces**
- Produces: `SurroundConfig.output_eq: Vec<EqBandConfig>` and `SurroundConfig.blocksize: Option<u32>` (both `#[serde(default)]`).
- Consumes: `EqBandConfig` (already defined in this file, lines 17–23: `{ kind: String, freq_hz: f32, q: f32, gain_db: f32 }`).

- [ ] **Step 1: Write the failing round-trip test.** Add to the test module in `crates/config/src/schema.rs`, mirroring `profile_ops.rs::eq_preset_round_trips_via_toml`:
  ```rust
  #[test]
  fn surround_config_round_trips_with_output_eq_and_blocksize() {
      let sc = SurroundConfig {
          enabled: true,
          hrir: Some("04-gsx-sennheiser-gsx".into()),
          channels: vec!["game".into()],
          hw_sink: None,
          mode: SurroundMode::Hrir71,
          crossfeed: 0,
          output_eq: vec![EqBandConfig { kind: "peaking".into(), freq_hz: 250.0, q: 1.0, gain_db: 3.0 }],
          blocksize: Some(128),
      };
      let toml = toml::to_string(&sc).expect("serialize");
      let back: SurroundConfig = toml::from_str(&toml).expect("deserialize");
      assert_eq!(sc, back);
  }

  #[test]
  fn surround_config_old_toml_defaults_output_eq_and_blocksize() {
      // An old config block missing both new fields must load with []/None.
      let old = "enabled = true\nhrir = \"04-gsx-sennheiser-gsx\"\nchannels = [\"game\"]\nmode = \"hrir71\"\ncrossfeed = 0\n";
      let sc: SurroundConfig = toml::from_str(old).expect("deserialize old");
      assert!(sc.output_eq.is_empty());
      assert_eq!(sc.blocksize, None);
  }
  ```
  Ensure the test module imports `EqBandConfig` and `SurroundMode` (`use super::*;` typically covers it; add explicit `use` if the module is selective).
- [ ] **Step 2: Run the test — expect FAIL.** `cargo test -p arctis-config surround_config_round_trips_with_output_eq_and_blocksize` → expected FAIL (compile error: no field `output_eq`/`blocksize` on `SurroundConfig`).
- [ ] **Step 3: Implement the two fields.** In the `SurroundConfig` struct, after the `crossfeed` field (line 367), add:
  ```rust
      /// Post-convolution EQ applied to the 2-ch binaural output. Empty = none.
      #[serde(default)]
      pub output_eq: Vec<EqBandConfig>,
      /// Convolver partition size (samples). None = PipeWire default.
      #[serde(default)]
      pub blocksize: Option<u32>,
  ```
  In `impl Default for SurroundConfig`, after `crossfeed: 0,` (line 378), add:
  ```rust
              output_eq: Vec::new(),
              blocksize: None,
  ```
- [ ] **Step 4: Run tests — expect PASS.** `cargo test -p arctis-config surround_config` → both new tests PASS. Then `cargo test -p arctis-config` → all green (existing `SurroundConfig` literals elsewhere in this crate's tests, if any construct it directly without `..Default::default()`, must be updated; grep `SurroundConfig {` in `crates/config` and add the two fields where a struct literal is exhaustive).
- [ ] **Step 5: Commit.**
  ```bash
  git add crates/config/src/schema.rs
  git commit -m "feat(config): add SurroundConfig.output_eq + blocksize (additive)"
  ```

---

### Task 2: Add the "DayZ Spatial" EQ preset

**Model:** haiku

**Files**
- Modify `crates/engine/src/presets.rs` — `factory_eq_presets()` vec (lines 17–36); test module (lines 87–126).

**Interfaces**
- Consumes band helpers (lines 7–13): `ls(g)` (lowshelf@31 q0.7), `pk(f,g)` (peaking q1.0), `pkq(f,q,g)`, `hs(g)` (highshelf@16k q0.7), `eqp(name,hint,bands)`.
- Produces: a new `EqPreset` named `"DayZ Spatial"` in the catalog (10 bands, no gain > +3 dB).

- [ ] **Step 1: Write the failing focused test.** Add to `mod tests` in `crates/engine/src/presets.rs`:
  ```rust
  #[test]
  fn dayz_spatial_preset_present_and_capped_at_plus_3() {
      let p = factory_eq_presets();
      let dz = p.iter().find(|p| p.name == "DayZ Spatial").expect("DayZ Spatial present");
      assert_eq!(dz.bands.len(), 10, "DayZ Spatial must have 10 bands");
      assert!(dz.bands.iter().all(|b| b.gain_db <= 3.0), "no band may boost beyond +3 dB");
      // 250 Hz footstep-weight boost and 62 Hz rumble cut are the signature moves.
      assert!(dz.bands.iter().any(|b| b.freq_hz == 250.0 && b.gain_db == 3.0));
      assert!(dz.bands.iter().any(|b| b.freq_hz == 62.0 && b.gain_db < 0.0));
  }
  ```
- [ ] **Step 2: Run the test — expect FAIL.** `cargo test -p arctis-engine presets::tests::dayz_spatial_preset_present_and_capped_at_plus_3` → FAIL (`DayZ Spatial present` panic).
- [ ] **Step 3: Implement the preset.** Insert into the `factory_eq_presets()` vec immediately after the `"FPS / Footsteps (Competitive)"` entry (after line 27, before `"Immersive"`):
  ```rust
          eqp("DayZ Spatial", "Gaming · post-HRIR footsteps + air",
              vec![ls(-1.0), pk(62.0,-3.0), pkq(125.0,1.41,-2.0), pk(250.0,3.0), pk(500.0,0.0), pk(1000.0,0.0), pk(2000.0,3.0), pk(4000.0,2.0), pk(8000.0,1.5), hs(1.5)]),
  ```
- [ ] **Step 4: Run tests — expect PASS.** `cargo test -p arctis-engine presets` → all green, including the existing `every_factory_preset_is_well_formed` (10 bands / canonical freqs / ±12 / shelves at band 0 & 9 / unique names — the new preset satisfies all) and `competitive_footsteps_preset_present_and_gentle` (untouched).
- [ ] **Step 5: Commit.**
  ```bash
  git add crates/engine/src/presets.rs
  git commit -m "feat(eq): add DayZ Spatial post-convolution EQ preset"
  ```

---

### Task 3: `convert::eq_model_from_bands` + `resolve_hrir_path_or_fallback`

**Model:** sonnet

**Files**
- Modify `crates/engine/src/convert.rs` — add `eq_model_from_bands` near `eq_model_for` (after line 72); add `FALLBACK_HRIR_STEM` const + `resolve_hrir_path_or_fallback` after `resolve_hrir_path` (after line 503). `SurroundConfig` is already imported (line 401).
- Modify `crates/engine/src/engine.rs` — add tests in the existing `mod tests` near the `resolve_hrir_path_*` tests (lines 6048–6129) and an `eq_model_from_bands` test near the `convert` eq tests.

**Interfaces**
- Produces: `pub fn eq_model_from_bands(bands: &[EqBandConfig]) -> Result<EqModel, EngineError>` (bands used as-is, **no** default overlay — unlike `eq_model_for`).
- Produces: `pub const FALLBACK_HRIR_STEM: &str = "07-oal+++-openal-max";` and `pub fn resolve_hrir_path_or_fallback(cfg: &SurroundConfig, base_dir: &Path) -> Result<(PathBuf, Option<String>), EngineError>` where the second tuple element is `Some(stem)` only when a fallback was substituted for a missing pinned stem.
- Consumes: existing `eq_band_from_cfg` (line 33), `resolve_hrir_path` (line 427, **unchanged**).

- [ ] **Step 1: Write the failing `eq_model_from_bands` test.** In `crates/engine/src/convert.rs` test module (the one starting ~line 553 with the `eq_band_from_cfg`/`eq_model_for` tests), add:
  ```rust
  #[test]
  fn eq_model_from_bands_uses_bands_verbatim_no_overlay() {
      let bands = vec![
          arctis_config::EqBandConfig { kind: "lowshelf".into(), freq_hz: 31.0, q: 0.7, gain_db: -1.0 },
          arctis_config::EqBandConfig { kind: "peaking".into(), freq_hz: 250.0, q: 1.0, gain_db: 3.0 },
      ];
      let model = super::eq_model_from_bands(&bands).expect("builds");
      assert_eq!(model.bands.len(), 2, "no default 10-band overlay");
      assert_eq!(model.bands[1].freq_hz, 250.0);
      assert_eq!(model.bands[1].gain_db, 3.0);
  }

  #[test]
  fn eq_model_from_bands_rejects_bad_kind() {
      let bands = vec![arctis_config::EqBandConfig { kind: "notakind".into(), freq_hz: 100.0, q: 1.0, gain_db: 0.0 }];
      assert!(super::eq_model_from_bands(&bands).is_err());
  }
  ```
- [ ] **Step 2: Run — expect FAIL.** `cargo test -p arctis-engine convert::tests::eq_model_from_bands_uses_bands_verbatim_no_overlay` → FAIL (no function `eq_model_from_bands`).
- [ ] **Step 3: Implement `eq_model_from_bands`.** In `crates/engine/src/convert.rs`, after `eq_model_for` (line 72):
  ```rust
  /// Build an `EqModel` from an explicit band curve (e.g. `SurroundConfig.output_eq`);
  /// bands are used as-is, with no default 10-band overlay (unlike `eq_model_for`).
  pub fn eq_model_from_bands(bands: &[EqBandConfig]) -> Result<EqModel, EngineError> {
      let bands = bands.iter().map(eq_band_from_cfg).collect::<Result<Vec<_>, _>>()?;
      Ok(EqModel { bands })
  }
  ```
- [ ] **Step 4: Run — expect PASS.** `cargo test -p arctis-engine convert::tests::eq_model_from_bands` → both PASS.
- [ ] **Step 5: Write the failing fallback tests.** In `crates/engine/src/engine.rs` test module, beside the `resolve_hrir_path_*` tests (after line 6129):
  ```rust
  #[test]
  fn resolve_or_fallback_present_stem_no_missing_flag() {
      let tmp = unique_cfg_tmp("hrir_or_fb_present");
      let base = tmp.join(convert::HRIR_BASE_SUBPATH);
      let profiles_dir = base.join("profiles");
      std::fs::create_dir_all(&profiles_dir).unwrap();
      std::fs::write(profiles_dir.join("04-gsx-sennheiser-gsx.wav"), b"").unwrap();
      let cfg = arctis_config::SurroundConfig { hrir: Some("04-gsx-sennheiser-gsx".into()), ..Default::default() };
      let (path, missing) = convert::resolve_hrir_path_or_fallback(&cfg, &base).expect("resolves");
      assert!(path.ends_with("04-gsx-sennheiser-gsx.wav"));
      assert_eq!(missing, None, "no fallback used → no missing flag");
      let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn resolve_or_fallback_missing_pinned_uses_bundled_and_reports_missing() {
      let tmp = unique_cfg_tmp("hrir_or_fb_missing");
      let base = tmp.join(convert::HRIR_BASE_SUBPATH);
      let profiles_dir = base.join("profiles");
      std::fs::create_dir_all(&profiles_dir).unwrap();
      // Pinned stem absent; bundled dry fallback present.
      std::fs::write(profiles_dir.join(format!("{}.wav", convert::FALLBACK_HRIR_STEM)), b"").unwrap();
      let cfg = arctis_config::SurroundConfig { hrir: Some("04-gsx-sennheiser-gsx".into()), ..Default::default() };
      let (path, missing) = convert::resolve_hrir_path_or_fallback(&cfg, &base).expect("falls back");
      assert!(path.ends_with(&format!("{}.wav", convert::FALLBACK_HRIR_STEM)));
      assert_eq!(missing, Some("04-gsx-sennheiser-gsx".to_string()));
      let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn resolve_or_fallback_missing_pinned_falls_back_to_any_available() {
      let tmp = unique_cfg_tmp("hrir_or_fb_any");
      let base = tmp.join(convert::HRIR_BASE_SUBPATH);
      let profiles_dir = base.join("profiles");
      std::fs::create_dir_all(&profiles_dir).unwrap();
      // Neither pinned nor bundled present, but another HRIR exists.
      std::fs::write(profiles_dir.join("99-other.wav"), b"").unwrap();
      let cfg = arctis_config::SurroundConfig { hrir: Some("04-gsx-sennheiser-gsx".into()), ..Default::default() };
      let (path, missing) = convert::resolve_hrir_path_or_fallback(&cfg, &base).expect("falls back to any");
      assert!(path.ends_with("99-other.wav"));
      assert_eq!(missing, Some("04-gsx-sennheiser-gsx".to_string()));
      let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn resolve_or_fallback_no_hrir_at_all_errors() {
      let tmp = unique_cfg_tmp("hrir_or_fb_none");
      let base = tmp.join(convert::HRIR_BASE_SUBPATH);
      std::fs::create_dir_all(&base).unwrap();
      let cfg = arctis_config::SurroundConfig { hrir: Some("04-gsx-sennheiser-gsx".into()), ..Default::default() };
      let result = convert::resolve_hrir_path_or_fallback(&cfg, &base);
      assert!(matches!(result, Err(EngineError::BadRequest(_))));
      let _ = std::fs::remove_dir_all(&tmp);
  }
  ```
- [ ] **Step 6: Run — expect FAIL.** `cargo test -p arctis-engine resolve_or_fallback` → FAIL (no function `resolve_hrir_path_or_fallback`).
- [ ] **Step 7: Implement the fallback wrapper.** In `crates/engine/src/convert.rs`, after `resolve_hrir_path` (after line 503), additive — do **not** modify `resolve_hrir_path`:
  ```rust
  /// Bundled dry HRIR used when a pinned stem is missing.
  pub const FALLBACK_HRIR_STEM: &str = "07-oal+++-openal-max";

  /// Resolve the HRIR path; if a pinned stem is missing, fall back to a bundled dry HRIR
  /// (then any available) and report the missing stem so the UI can prompt to import.
  /// Returns `(path, missing_stem)`. `missing_stem = Some(stem)` only when a fallback was used.
  pub fn resolve_hrir_path_or_fallback(
      cfg: &SurroundConfig,
      base_dir: &std::path::Path,
  ) -> Result<(std::path::PathBuf, Option<String>), crate::error::EngineError> {
      match resolve_hrir_path(cfg, base_dir) {
          Ok(p) => Ok((p, None)),
          Err(e) => {
              if let Some(stem) = &cfg.hrir {
                  for fb in [Some(FALLBACK_HRIR_STEM.to_string()), None] {
                      let fb_cfg = SurroundConfig { hrir: fb, ..cfg.clone() };
                      if let Ok(p) = resolve_hrir_path(&fb_cfg, base_dir) {
                          return Ok((p, Some(stem.clone())));
                      }
                  }
              }
              Err(e)
          }
      }
  }
  ```
- [ ] **Step 8: Run — expect PASS.** `cargo test -p arctis-engine resolve_or_fallback` → all 4 PASS. Then `cargo test -p arctis-engine convert` and `cargo test -p arctis-engine resolve_hrir_path` → existing tests still green.
- [ ] **Step 9: Commit.**
  ```bash
  git add crates/engine/src/convert.rs crates/engine/src/engine.rs
  git commit -m "feat(engine): eq_model_from_bands + resolve_hrir_path_or_fallback"
  ```

---

### Task 4: Thread `blocksize` through the surround renderer

**Model:** sonnet

**Files**
- Modify `crates/audio/src/surround.rs` — `struct SurroundRender` (lines 42–51); convolver emission loop (lines 186–190); `render_surround_conf` literal (lines 595–600); `recreate_ex` (lines 718–732); the three `SurroundRender { .. }` test literals (lines ~906–911, 924–929, 941–946).

**Interfaces**
- Produces: `SurroundRender.blocksize: Option<u32>` (new field) and `recreate_ex(&mut self, hrir_path, channels, output_eq, blocksize: Option<u32>)` (new 4th param).
- Consumes: nothing new. When `blocksize == None`, every existing snapshot/fixture stays byte-identical.

- [ ] **Step 1: Write the failing blocksize snapshot test.** Add to `mod tests` in `crates/audio/src/surround.rs` (after `render_5_1_emits_6_channel_capture_and_no_side_nodes`):
  ```rust
  #[test]
  fn render_with_blocksize_emits_blocksize_on_every_convolver() {
      let spec = test_spec();
      let got = render_surround_conf_ex(&SurroundRender {
          spec: &spec,
          hrir_path: &PathBuf::from("/test/hrir.wav"),
          channels: 8,
          output_eq: None,
          blocksize: Some(128),
      })
      .unwrap();
      let conv_lines: Vec<&str> = got.lines().filter(|l| l.contains("label = convolver")).collect();
      assert_eq!(conv_lines.len(), 16, "7.1 has 16 convolvers");
      for line in &conv_lines {
          assert!(line.contains("blocksize = 128"), "convolver line missing blocksize: {line}");
      }
  }

  #[test]
  fn render_without_blocksize_omits_blocksize() {
      let spec = test_spec();
      let got = render_surround_conf_ex(&SurroundRender {
          spec: &spec,
          hrir_path: &PathBuf::from("/test/hrir.wav"),
          channels: 8,
          output_eq: None,
          blocksize: None,
      })
      .unwrap();
      assert!(!got.contains("blocksize"), "None must not emit a blocksize key");
  }
  ```
- [ ] **Step 2: Run — expect FAIL.** `cargo test -p arctis-audio render_with_blocksize_emits_blocksize_on_every_convolver` → FAIL (missing field `blocksize` in `SurroundRender`).
- [ ] **Step 3: Add the field.** In `struct SurroundRender` (after `output_eq` at line 50):
  ```rust
      /// Convolver partition size in samples. `Some(n)` emits `blocksize = n` into
      /// every convolver node's config; `None` omits it (PipeWire default).
      pub blocksize: Option<u32>,
  ```
- [ ] **Step 4: Emit blocksize in the convolver loop.** Replace the loop at lines 186–190 with:
  ```rust
      for (name, ch) in convs {
          let bs = match r.blocksize {
              Some(n) => format!("  blocksize = {n}"),
              None => String::new(),
          };
          out.push_str(&format!(
              "                    {{ type = builtin  label = convolver  name = {name}  config = {{ filename = \"{hrir_str}\"  channel = {ch}{bs} }} }}\n"
          ));
      }
  ```
- [ ] **Step 5: Fix all `SurroundRender` literals.** `render_surround_conf` (lines 595–600) add `blocksize: None,`. The three test literals at lines ~906–911 (`render_surround_conf_ex_rejects_invalid_channels`), ~924–929 (`render_surround_conf_ex_rejects_invalid_eq`), ~941–946 (`render_5_1_emits_6_channel_capture_and_no_side_nodes`) each add `blocksize: None,`. In `recreate_ex` (lines 718–732), change the signature and thread the value:
  ```rust
      pub fn recreate_ex(
          &mut self,
          hrir_path: &Path,
          channels: u8,
          output_eq: Option<&EqModel>,
          blocksize: Option<u32>,
      ) -> Result<ConfHandle, AudioError> {
          self.remove()?;
          let conf = render_surround_conf_ex(&SurroundRender {
              spec: &self.spec,
              hrir_path,
              channels,
              output_eq,
              blocksize,
          })?;
          self.spawn_conf(conf)
      }
  ```
  (Leave `recreate_stereo_bypass` unchanged — it has no convolver.)
- [ ] **Step 6: Run tests — expect PASS.** `cargo test -p arctis-audio surround` → all green. The `render_surround_conf_matches_fixture` test (line 800) still passes byte-for-byte because `render_surround_conf` passes `blocksize: None` → no emission. Any in-crate `recreate_ex` callers (grep `recreate_ex` in `crates/audio`) get the new arg.
- [ ] **Step 7: Commit.**
  ```bash
  git add crates/audio/src/surround.rs
  git commit -m "feat(audio): parameterize convolver blocksize in surround renderer"
  ```

---

### Task 5: Engine state plumbing — `surround_hrir_missing` field + snapshot

**Model:** sonnet

**Files**
- Modify `crates/engine/src/engine.rs` — `Engine` struct (lines 174–203), `Engine::new` (lines 206–225), `Engine::with_probe` (lines 230–248), `state()` `SurroundSnapshot` builder (lines 598–620).
- Modify `crates/engine/src/state.rs` — `SurroundSnapshot` (lines 139–160).

**Interfaces**
- Produces: `Engine.surround_hrir_missing: Option<String>` (runtime field, init `None`); `SurroundSnapshot.hrir_missing: Option<String>`, `SurroundSnapshot.output_eq: Vec<EqBandSnapshot>`, `SurroundSnapshot.blocksize: Option<u32>` (all `#[serde(default)]`).
- Consumes: `EqBandSnapshot` (already in `state.rs`, lines 221–226) and `SurroundConfig.output_eq/blocksize` (Task 1).

- [ ] **Step 1: Write the failing snapshot test.** Add to `mod tests` in `crates/engine/src/state.rs` (or a new test if none exist there) verifying defaults and serde:
  ```rust
  #[test]
  fn surround_snapshot_defaults_have_no_missing_and_empty_output_eq() {
      let s = SurroundSnapshot::default();
      assert_eq!(s.hrir_missing, None);
      assert!(s.output_eq.is_empty());
      assert_eq!(s.blocksize, None);
  }

  #[test]
  fn surround_snapshot_old_json_defaults_new_fields() {
      // JSON from an older engine omits the three new fields.
      let json = r#"{"enabled":false,"hrir":null,"available_hrirs":[],"channels":[],"hw_sink":null}"#;
      let s: SurroundSnapshot = serde_json::from_str(json).expect("deserialize old snapshot");
      assert_eq!(s.hrir_missing, None);
      assert!(s.output_eq.is_empty());
      assert_eq!(s.blocksize, None);
  }
  ```
  (If `serde_json` is not a dev-dep of `arctis-engine`, drop the second test or keep only the `Default` test — confirm with `grep serde_json crates/engine/Cargo.toml`; it is used elsewhere in engine tests.)
- [ ] **Step 2: Run — expect FAIL.** `cargo test -p arctis-engine surround_snapshot_defaults_have_no_missing_and_empty_output_eq` → FAIL (no field `hrir_missing`).
- [ ] **Step 3: Extend `SurroundSnapshot`.** In `crates/engine/src/state.rs`, after `negotiated_channels` (line 159) add:
  ```rust
      /// Pinned HRIR stem that was requested but not installed (a fallback is in use).
      /// `None` = the pinned/selected HRIR resolved normally. UI shows an import prompt when set.
      #[serde(default)]
      pub hrir_missing: Option<String>,
      /// Explicit post-convolution EQ on the binaural tail. Empty = none / legacy relocation.
      #[serde(default)]
      pub output_eq: Vec<EqBandSnapshot>,
      /// Convolver partition size, if pinned by the profile.
      #[serde(default)]
      pub blocksize: Option<u32>,
  ```
- [ ] **Step 4: Add the engine runtime field.** In `crates/engine/src/engine.rs`, in `struct Engine` after `last_volume_write` (line 202):
  ```rust
      /// Set by `apply_surround` when a pinned HRIR stem was missing and a fallback was
      /// substituted; surfaced in `state().surround.hrir_missing` so the UI can prompt to import.
      surround_hrir_missing: Option<String>,
  ```
  In `Engine::new` (after `last_volume_write: None,` line 223) and `Engine::with_probe` (after line 246) add `surround_hrir_missing: None,`.
- [ ] **Step 5: Surface the fields in `state()`.** In `crates/engine/src/engine.rs`, just before the `let surround = if let Ok(p) = self.config.active()` block (line 598), capture the runtime flag (avoids any borrow overlap with `self.config.active()`):
  ```rust
          let surround_hrir_missing = self.surround_hrir_missing.clone();
  ```
  Then inside the `crate::state::SurroundSnapshot { .. }` literal (lines 607–617), after `negotiated_channels: None,` add:
  ```rust
                  hrir_missing: surround_hrir_missing,
                  output_eq: sc.output_eq.iter().map(|b| crate::state::EqBandSnapshot {
                      kind: b.kind.clone(),
                      freq_hz: b.freq_hz,
                      q: b.q,
                      gain_db: b.gain_db,
                  }).collect(),
                  blocksize: sc.blocksize,
  ```
  (The `else { SurroundSnapshot::default() }` branch already yields `None`/empty for the new fields via `#[serde(default)]` + `Default`.)
- [ ] **Step 6: Run tests — expect PASS.** `cargo test -p arctis-engine surround_snapshot` and `cargo test -p arctis-engine state` → green. `cargo test -p arctis-engine` → still compiles (no apply_surround changes yet; the field is currently only read, never written — that is fine and warning-free because it is read in `state()`).
- [ ] **Step 7: Commit.**
  ```bash
  git add crates/engine/src/engine.rs crates/engine/src/state.rs
  git commit -m "feat(engine): surround hrir_missing/output_eq/blocksize in state snapshot"
  ```

---

### Task 6: `apply_surround` — explicit output_eq, HRIR fallback flag, blocksize

**Model:** sonnet

**Files**
- Modify `crates/engine/src/engine.rs` — `apply_surround` (lines 2359–2570): disabled branch (line 2366), output_eq derivation (lines 2416–2428), HRIR resolution (lines 2433–2446), `recreate_ex` calls (lines 2462, 2472), `recreate_stereo_bypass` branch (line 2475).

**Interfaces**
- Consumes: `convert::eq_model_from_bands` (Task 3), `convert::resolve_hrir_path_or_fallback` (Task 3), `recreate_ex(.., blocksize)` (Task 4), `Engine.surround_hrir_missing` (Task 5), `SurroundConfig.output_eq/blocksize` (Task 1).
- Produces: post-convolution EQ sourced from `sc.output_eq` when non-empty; `self.surround_hrir_missing` set each apply; `sc.blocksize` reaching the renderer.

- [ ] **Step 1: Write the failing apply test (output_eq preferred).** Add to `mod tests` in `crates/engine/src/engine.rs`, mirroring the existing apply_surround test setup (grep `apply_surround` in the test module for the established harness — `MockRunner`/`with_probe`/`seed_pw_version` and a temp HRIR base via `unique_cfg_tmp` + `HRIR_BASE_SUBPATH`). The test enables surround with an explicit `output_eq` and a present HRIR, then asserts `state().surround.hrir_missing == None` and the spawned conf carries `blocksize = 128` and the post-EQ nodes:
  ```rust
  #[test]
  fn apply_surround_uses_explicit_output_eq_and_blocksize_and_clears_missing() {
      // Arrange a profile with surround enabled, a present pinned HRIR, blocksize 128,
      // and an explicit output_eq. (Use the same Engine+MockRunner harness as the other
      // apply_surround tests in this module; set HOME/base via the test's HRIR temp dir.)
      // ... build `engine` with an active profile whose surround = {
      //        enabled: true, hrir: Some("g"), channels: ["game"], mode: Hrir71,
      //        blocksize: Some(128), output_eq: vec![ peaking 250 +3 ], .. }
      //     and write profiles/g.wav into the temp base.
      // Act:
      let profile = engine.config.active().unwrap().clone();
      engine.apply_surround(&profile).unwrap();
      // Assert: no missing flag (HRIR present).
      assert_eq!(engine.state().surround.hrir_missing, None);
      // Assert: the last spawned surround conf carried blocksize=128 and per-ear EQ nodes.
      let conf = engine.runner_last_surround_conf(); // helper: find the written /tmp/arctis_*.conf via MockRunner spawn args
      assert!(conf.contains("blocksize = 128"));
      assert!(conf.contains("eq_l_0"));
  }
  ```
  > Implementation note for the worker: reuse whatever assertion mechanism the existing apply_surround tests use to inspect the spawned conf (MockRunner records `spawn_owned("pipewire", ["-c", path])`; read the file at `path`, or assert on recorded args). If no such helper exists, assert via the `SurroundBackend` conf path `/tmp/arctis_arctis_surround.conf`. Keep the test hermetic and `#[cfg(test)]`-only (unwrap allowed).
- [ ] **Step 2: Run — expect FAIL.** `cargo test -p arctis-engine apply_surround_uses_explicit_output_eq_and_blocksize_and_clears_missing` → FAIL (explicit output_eq not yet wired; conf has no blocksize).
- [ ] **Step 3: Write the failing missing-HRIR test.** Same harness, but the pinned stem is absent while the bundled fallback `07-oal+++-openal-max.wav` is present:
  ```rust
  #[test]
  fn apply_surround_missing_pinned_hrir_falls_back_and_sets_flag() {
      // profiles/ contains only 07-oal+++-openal-max.wav; surround.hrir = Some("04-gsx-sennheiser-gsx").
      let profile = engine.config.active().unwrap().clone();
      engine.apply_surround(&profile).unwrap();
      assert_eq!(engine.state().surround.hrir_missing, Some("04-gsx-sennheiser-gsx".to_string()));
  }
  ```
- [ ] **Step 4: Run — expect FAIL.** `cargo test -p arctis-engine apply_surround_missing_pinned_hrir_falls_back_and_sets_flag` → FAIL (flag never set).
- [ ] **Step 5: Implement the apply_surround changes.**
  (a) Disabled branch — clear the flag. At the top of the `if !sc.enabled {` block (line 2367), add:
  ```rust
          self.surround_hrir_missing = None;
  ```
  (b) Prefer explicit `sc.output_eq`. Replace the `let output_eq: Option<EqModel> = ...` block (lines 2416–2428) with:
  ```rust
          // Post-convolution binaural EQ: prefer an explicit profile curve; otherwise fall back
          // to relocating the primary surround channel's EQ to the tail (legacy back-compat).
          let output_eq: Option<EqModel> = if !sc.output_eq.is_empty() {
              Some(convert::eq_model_from_bands(&sc.output_eq)?)
          } else if let Some(ref ch_id) = primary_ch_id {
              if let Some(ch) = profile.channels.iter().find(|c| &c.id == ch_id) {
                  if !ch.eq.is_empty() { Some(convert::eq_model_for(ch)?) } else { None }
              } else {
                  None
              }
          } else {
              None
          };
  ```
  (c) HRIR resolution with fallback flag. Replace the `hrir_path_opt` match arm body (lines 2435–2445) — the `_ =>` arm — with a version that uses `resolve_hrir_path_or_fallback` and records the missing stem. Also default the flag to `None` for the StereoBypass arm:
  ```rust
          let effective = resolve_effective_mode(sc.mode, None);
          let hrir_path_opt: Option<std::path::PathBuf> = match effective {
              SurroundMode::StereoBypass => {
                  self.surround_hrir_missing = None;
                  None
              }
              _ => match convert::hrir_base_dir()
                  .and_then(|base| convert::resolve_hrir_path_or_fallback(sc, &base))
              {
                  Ok((p, missing)) => {
                      self.surround_hrir_missing = missing;
                      Some(p)
                  }
                  Err(e) => {
                      eprintln!("warning: apply_surround HRIR resolve failed (skipping surround): {e}");
                      self.surround_hrir_missing = None;
                      return Ok(());
                  }
              },
          };
  ```
  (d) Thread `sc.blocksize` into both `recreate_ex` calls. Line 2462 becomes `surround_be.recreate_ex(hrir_path, 8, output_eq.as_ref(), sc.blocksize)`; line 2472 becomes `surround_be.recreate_ex(hrir_path, 6, output_eq.as_ref(), sc.blocksize)`. The `recreate_stereo_bypass` call (line 2475) is unchanged.
  (e) The "flatten primary channel sink when `output_eq.is_some()`" logic (line 2544) stays **unchanged** — it already keys off `output_eq.is_some()`, which now also covers the explicit-curve case.
- [ ] **Step 6: Run tests — expect PASS.** `cargo test -p arctis-engine apply_surround` → both new tests PASS and all existing apply_surround tests stay green (legacy relocation path still fires when `sc.output_eq` is empty; `recreate_stereo_bypass` untouched).
- [ ] **Step 7: Commit.**
  ```bash
  git add crates/engine/src/engine.rs
  git commit -m "feat(engine): explicit output_eq + HRIR fallback flag + blocksize in apply_surround"
  ```

---

### Task 7: Data-driven factory-profile catalog

**Model:** sonnet

**Files**
- Modify `crates/engine/src/factory_profiles.rs` — replace `dayz_profile` (lines 20–44) and rewrite the 4 tests (lines 87–137); keep `make_active()` helper (lines 51–85).

**Interfaces**
- Produces: `pub struct ChannelEqSeed`, `pub struct FactoryProfileSpec`, `const DAYZ`, `pub fn factory_profiles() -> &'static [FactoryProfileSpec]`, `pub fn find_factory_profile(name: &str) -> Option<&'static FactoryProfileSpec>`, `pub fn apply_factory_spec(active: &Profile, spec: &FactoryProfileSpec) -> Result<Profile, EngineError>`.
- Consumes: `crate::presets::factory_eq_presets()` (preset name → bands), `arctis_config::{Profile, SurroundMode, EqBandConfig}`, `crate::error::EngineError`.

- [ ] **Step 1: Write the failing catalog tests.** Replace the entire `#[cfg(test)] mod tests` (lines 46–138) with the updated `make_active()` (unchanged body) plus:
  ```rust
  #[test]
  fn apply_dayz_spec_sets_surround_and_output_eq() {
      let active = make_active();
      let p = apply_factory_spec(&active, &DAYZ).unwrap();
      assert_eq!(p.name, "DayZ");
      assert!(p.surround.enabled);
      assert_eq!(p.surround.channels, vec!["game".to_string()]);
      assert_eq!(p.surround.hrir, Some("04-gsx-sennheiser-gsx".to_string()));
      assert_eq!(p.surround.mode, SurroundMode::Hrir71);
      assert_eq!(p.surround.blocksize, Some(128));
      assert_eq!(p.surround.output_eq.len(), 10, "DayZ Spatial seeds 10 post bands");
      assert_eq!(p.default_sink_channel, Some("game".into()));
      // content_eq is None for DayZ → game channel EQ stays empty.
      let game = p.channels.iter().find(|c| c.id == "game").unwrap();
      assert!(game.eq.is_empty(), "no pre-convolution channel EQ for DayZ");
  }

  #[test]
  fn apply_dayz_spec_preserves_node_names_and_chat() {
      let active = make_active();
      let p = apply_factory_spec(&active, &DAYZ).unwrap();
      assert_eq!(p.channels.iter().find(|c| c.id == "game").unwrap().node_name, "Arctis_Game");
      assert_eq!(p.channels.iter().find(|c| c.id == "chat").unwrap().node_name, "Arctis_Chat");
      assert!(p.channels.iter().find(|c| c.id == "chat").unwrap().eq.is_empty());
  }

  #[test]
  fn apply_dayz_spec_overrides_hrir_but_preserves_hw_sink() {
      let mut active = make_active();
      active.surround.hw_sink = Some("alsa_output.pci-0000_00_1f.3".into());
      active.surround.hrir = Some("00-default-asm".into());
      let p = apply_factory_spec(&active, &DAYZ).unwrap();
      assert_eq!(p.surround.hw_sink, Some("alsa_output.pci-0000_00_1f.3".into()), "hw_sink preserved");
      assert_eq!(p.surround.hrir, Some("04-gsx-sennheiser-gsx".into()), "hrir pinned by spec");
      assert_eq!(p.surround.channels, vec!["game".to_string()]);
  }

  #[test]
  fn find_factory_profile_is_case_insensitive() {
      assert!(find_factory_profile("dayz").is_some());
      assert!(find_factory_profile("DAYZ").is_some());
      assert!(find_factory_profile("DayZ").is_some());
      assert!(find_factory_profile("nope").is_none());
  }

  #[test]
  fn apply_factory_spec_unknown_preset_errors() {
      let active = make_active();
      let bogus = FactoryProfileSpec { output_eq_preset: Some("No Such Preset"), ..DAYZ };
      assert!(matches!(apply_factory_spec(&active, &bogus), Err(EngineError::BadRequest(_))));
  }
  ```
  (The `..DAYZ` struct-update syntax requires `FactoryProfileSpec` fields to be `Copy`-able by value; since all fields are `&'static` / `Copy` / `Option<&'static …>`, `..DAYZ` works without `Clone`.)
- [ ] **Step 2: Run — expect FAIL.** `cargo test -p arctis-engine factory_profiles` → FAIL (no `apply_factory_spec`, `DAYZ`, `FactoryProfileSpec`, `find_factory_profile`).
- [ ] **Step 3: Implement the catalog.** Replace the module body above the tests (lines 1–44) with:
  ```rust
  //! Data-driven factory-profile catalog. Each `FactoryProfileSpec` is the set of
  //! overrides a template applies onto a clone of the active profile, so node names,
  //! hw_sink, mic chain, and master volume are preserved. Adding a future game =
  //! one struct literal, no new code paths.

  use arctis_config::{Profile, SurroundMode};
  use crate::error::EngineError;

  /// One channel's pre-spatial content EQ seed (a named preset applied to a channel).
  pub struct ChannelEqSeed {
      pub channel_id: &'static str,
      pub preset_name: &'static str,
  }

  /// A factory profile template: overrides applied onto a clone of the active profile.
  pub struct FactoryProfileSpec {
      pub name: &'static str,
      pub hrir_stem: Option<&'static str>,
      pub mode: SurroundMode,
      pub blocksize: Option<u32>,
      pub surround_channels: &'static [&'static str],
      pub default_sink_channel: Option<&'static str>,
      pub content_eq: Option<ChannelEqSeed>,
      pub output_eq_preset: Option<&'static str>,
  }

  const DAYZ: FactoryProfileSpec = FactoryProfileSpec {
      name: "DayZ",
      hrir_stem: Some("04-gsx-sennheiser-gsx"),
      mode: SurroundMode::Hrir71,
      blocksize: Some(128),
      surround_channels: &["game"],
      default_sink_channel: Some("game"),
      content_eq: None,
      output_eq_preset: Some("DayZ Spatial"),
  };

  /// The catalog. DayZ is today's only entry.
  pub fn factory_profiles() -> &'static [FactoryProfileSpec] {
      const ALL: &[FactoryProfileSpec] = &[DAYZ];
      ALL
  }

  /// Look up a template by name, case-insensitive.
  pub fn find_factory_profile(name: &str) -> Option<&'static FactoryProfileSpec> {
      factory_profiles().iter().find(|s| s.name.eq_ignore_ascii_case(name))
  }

  /// Resolve a named EQ preset to its bands; unknown names are a hard error.
  fn preset_bands(name: &str) -> Result<Vec<arctis_config::EqBandConfig>, EngineError> {
      crate::presets::factory_eq_presets()
          .into_iter()
          .find(|p| p.name == name)
          .map(|p| p.bands)
          .ok_or_else(|| EngineError::BadRequest(format!("unknown factory EQ preset: {name}")))
  }

  /// Apply a template onto a clone of the active profile, preserving hardware-specific
  /// settings (node names, hw_sink, mic chain, master volume).
  pub fn apply_factory_spec(active: &Profile, spec: &FactoryProfileSpec) -> Result<Profile, EngineError> {
      let mut profile = active.clone();
      profile.name = spec.name.into();

      if let Some(seed) = &spec.content_eq {
          let bands = preset_bands(seed.preset_name)?;
          if let Some(ch) = profile.channels.iter_mut().find(|c| c.id == seed.channel_id) {
              ch.eq = bands;
          }
      }

      let mut surround = active.surround.clone();
      surround.enabled = true;
      surround.channels = spec.surround_channels.iter().map(|s| s.to_string()).collect();
      surround.mode = spec.mode;
      surround.blocksize = spec.blocksize;
      if let Some(stem) = spec.hrir_stem {
          surround.hrir = Some(stem.into());
      }
      surround.output_eq = match spec.output_eq_preset {
          Some(n) => preset_bands(n)?,
          None => Vec::new(),
      };
      profile.surround = surround;
      profile.default_sink_channel = spec.default_sink_channel.map(|s| s.to_string());
      Ok(profile)
  }
  ```
- [ ] **Step 4: Run tests — expect PASS.** `cargo test -p arctis-engine factory_profiles` → all green.
- [ ] **Step 5: Commit.**
  ```bash
  git add crates/engine/src/factory_profiles.rs
  git commit -m "feat(engine): data-driven factory-profile catalog (DayZ first entry)"
  ```

---

### Task 8: Make `create_factory_profile` catalog-driven

**Model:** haiku

**Files**
- Modify `crates/engine/src/engine.rs` — `create_factory_profile` (lines 1167–1185).

**Interfaces**
- Consumes: `factory_profiles::find_factory_profile`, `factory_profiles::apply_factory_spec` (Task 7).
- Produces: unchanged public behavior — `create_factory_profile("dayz")` upserts a "DayZ" profile, activates, saves, reconciles, emits `ProfileCreated { name: "DayZ" }`.

- [ ] **Step 1: Confirm the existing create test.** `grep -n "create_factory_profile" crates/engine/src/engine.rs` to find the existing test (likely `create_factory_profile_dayz_*`). Read it; it should create `"dayz"` then assert `active_profile == "DayZ"`. Keep it as the failing/passing guard — do not delete it.
- [ ] **Step 2: Run — expect PASS (baseline) then change.** `cargo test -p arctis-engine create_factory_profile` → currently PASS (old hardcoded arm). This is the regression guard.
- [ ] **Step 3: Implement the catalog-driven body.** Replace lines 1167–1185 with:
  ```rust
      pub fn create_factory_profile(&mut self, template: &str) -> Result<(), EngineError> {
          let spec = crate::factory_profiles::find_factory_profile(template)
              .ok_or_else(|| EngineError::BadRequest(format!("unknown factory profile template: {template}")))?;
          let active = self.config.active()?.clone();
          let p = crate::factory_profiles::apply_factory_spec(&active, spec)?;
          let name = p.name.clone();
          self.config.upsert_profile(p);
          self.config.active_profile = name.clone();
          self.save_config()?;
          self.reconcile()?;
          self.emit(Event::ProfileCreated { name });
          Ok(())
      }
  ```
  (Update the doc comment at lines 1160–1166 if it lists supported templates — keep it accurate but generic: "templates come from `factory_profiles::factory_profiles()`".)
- [ ] **Step 4: Add a coverage test for the new shape.** Add beside the existing test:
  ```rust
  #[test]
  fn create_factory_profile_unknown_template_errors() {
      // ... build a minimal engine (same harness as the existing create test) ...
      let err = engine.create_factory_profile("not-a-game").unwrap_err();
      assert!(matches!(err, EngineError::BadRequest(_)));
  }
  ```
- [ ] **Step 5: Run tests — expect PASS.** `cargo test -p arctis-engine create_factory_profile` → existing DayZ test stays green (now routed through the catalog) and the unknown-template test passes.
- [ ] **Step 6: Commit.**
  ```bash
  git add crates/engine/src/engine.rs
  git commit -m "refactor(engine): create_factory_profile is catalog-driven"
  ```

> **End of Part A.** `cargo test --workspace` should be green and `profileCreateFromFactory("DayZ")` now produces the full competitive profile end-to-end (verify manually after a daemon restart).

---

# Part B — Tauri + UI

> Tasks 9–14. Data-driven factory listing, post-convolution EQ controls, and the missing-HRIR import prompt. IPC flows through `arctis-client::Request` → daemon dispatch → engine, with Tauri commands as thin forwarders (existing pattern in `src-tauri/src/commands.rs`).

---

### Task 9: Protocol + engine — `FactoryProfileInfo` and the three new requests

**Model:** sonnet

**Files**
- Modify `crates/engine/src/factory_profiles.rs` — add `FactoryProfileInfo` + a builder.
- Modify `crates/engine/src/engine.rs` — add `Engine::list_factory_profiles`.
- Modify `crates/engine/src/lib.rs` — re-export `FactoryProfileInfo` (line 20–24 `pub use`).
- Modify `crates/client/src/protocol.rs` — `Request` enum (lines 44–231): add `ListFactoryProfiles`, `SurroundSetOutputEq`, `SurroundSetBlocksize`; `Response` struct (lines 234–255) + constructors: add `factory_profiles` payload; wire-tag/round-trip tests (near line 581).

**Interfaces**
- Produces: `pub struct FactoryProfileInfo { name: String, hrir: Option<String>, mode: String }` (Serialize/Deserialize); `Engine::list_factory_profiles(&self) -> Vec<FactoryProfileInfo>`; `Request::ListFactoryProfiles`, `Request::SurroundSetOutputEq { bands: Vec<arctis_engine::EqBandSnapshot> }`, `Request::SurroundSetBlocksize { blocksize: Option<u32> }`; `Response.factory_profiles: Option<Vec<arctis_engine::FactoryProfileInfo>>` + `Response::ok_with_factory_profiles`.
- Consumes: `factory_profiles::factory_profiles()`, `surround_mode_str` (engine.rs line 138), `EqBandSnapshot` (re-exported from `arctis_engine`).

- [ ] **Step 1: Write the failing `list_factory_profiles` engine test.** In `crates/engine/src/factory_profiles.rs` tests (or engine.rs tests where Engine is constructed):
  ```rust
  // in factory_profiles.rs tests:
  #[test]
  fn factory_profile_info_lists_dayz() {
      let infos: Vec<FactoryProfileInfo> = factory_profiles()
          .iter()
          .map(|s| FactoryProfileInfo {
              name: s.name.to_string(),
              hrir: s.hrir_stem.map(|h| h.to_string()),
              mode: format!("{:?}", s.mode),
          })
          .collect();
      assert!(infos.iter().any(|i| i.name == "DayZ" && i.hrir.as_deref() == Some("04-gsx-sennheiser-gsx")));
  }
  ```
- [ ] **Step 2: Run — expect FAIL.** `cargo test -p arctis-engine factory_profile_info_lists_dayz` → FAIL (no `FactoryProfileInfo`).
- [ ] **Step 3: Implement `FactoryProfileInfo` + engine method.** In `factory_profiles.rs`, after the catalog functions:
  ```rust
  /// Serializable summary of a factory template for the UI listing.
  #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
  pub struct FactoryProfileInfo {
      pub name: String,
      pub hrir: Option<String>,
      pub mode: String,
  }
  ```
  In `engine.rs`, add to `impl<R: CommandRunner> Engine<R>` (near `create_factory_profile`):
  ```rust
      /// List the factory-profile catalog as serializable info for the UI.
      pub fn list_factory_profiles(&self) -> Vec<crate::factory_profiles::FactoryProfileInfo> {
          crate::factory_profiles::factory_profiles()
              .iter()
              .map(|s| crate::factory_profiles::FactoryProfileInfo {
                  name: s.name.to_string(),
                  hrir: s.hrir_stem.map(|h| h.to_string()),
                  mode: surround_mode_str(s.mode).to_string(),
              })
              .collect()
      }
  ```
  In `lib.rs`, add `FactoryProfileInfo` to the exports: `pub use factory_profiles::FactoryProfileInfo;` (new line after line 17).
- [ ] **Step 4: Run — expect PASS.** `cargo test -p arctis-engine factory_profile_info_lists_dayz` → PASS.
- [ ] **Step 5: Write the failing protocol round-trip tests.** In `crates/client/src/protocol.rs` tests (near line 581), mirror the `ProfileCreateFromFactory` wire-tag tests:
  ```rust
  #[test]
  fn list_factory_profiles_wire_tag() {
      let req = Request::ListFactoryProfiles;
      let json = serde_json::to_string(&req).unwrap();
      assert!(json.contains("\"cmd\":\"list-factory-profiles\""));
      let back: Request = serde_json::from_str(&json).unwrap();
      assert_eq!(back, Request::ListFactoryProfiles);
  }

  #[test]
  fn surround_set_output_eq_round_trips() {
      let req = Request::SurroundSetOutputEq {
          bands: vec![arctis_engine::EqBandSnapshot { kind: "peaking".into(), freq_hz: 250.0, q: 1.0, gain_db: 3.0 }],
      };
      let json = serde_json::to_string(&req).unwrap();
      assert!(json.contains("\"cmd\":\"surround-set-output-eq\""));
      let back: Request = serde_json::from_str(&json).unwrap();
      assert_eq!(back, req);
  }

  #[test]
  fn surround_set_blocksize_round_trips() {
      let req = Request::SurroundSetBlocksize { blocksize: Some(128) };
      let json = serde_json::to_string(&req).unwrap();
      assert!(json.contains("\"cmd\":\"surround-set-blocksize\""));
      let back: Request = serde_json::from_str(&json).unwrap();
      assert_eq!(back, req);
  }
  ```
  (`EqBandSnapshot` derives `PartialEq` so the `Request` `PartialEq` derive holds.)
- [ ] **Step 6: Run — expect FAIL.** `cargo test -p arctis-client list_factory_profiles_wire_tag` → FAIL (no such variant).
- [ ] **Step 7: Implement the protocol additions.** In the `Request` enum (before the closing `}` at line 231), add:
  ```rust
      /// List the static factory-profile catalog (name + hrir + mode). Returns
      /// `Response.factory_profiles`.
      ListFactoryProfiles,
      /// Set the explicit post-convolution surround EQ on the active profile's binaural tail.
      SurroundSetOutputEq { bands: Vec<arctis_engine::EqBandSnapshot> },
      /// Pin (or clear) the convolver blocksize on the active profile's surround.
      SurroundSetBlocksize { blocksize: Option<u32> },
  ```
  In `Response` (after `coexist_result`, line 254), add:
  ```rust
      /// Factory-profile catalog payload. Populated only for ListFactoryProfiles responses.
      #[serde(skip_serializing_if = "Option::is_none")]
      pub factory_profiles: Option<Vec<arctis_engine::FactoryProfileInfo>>,
  ```
  Update **every** `Response` constructor (`ok_with_state`, `ok_with_text`, `err`, `ok_with_coexist_report`, and any others — grep `factory_profiles: ` is the surest check) to add `factory_profiles: None,`, and add a new constructor:
  ```rust
      pub fn ok_with_factory_profiles(profiles: Vec<arctis_engine::FactoryProfileInfo>) -> Self {
          Self {
              ok: true,
              state: None,
              error: None,
              text: None,
              streams: None,
              output_devices: None,
              coexist_report: None,
              coexist_result: None,
              factory_profiles: Some(profiles),
          }
      }
  ```
  (Add `factory_profiles: None` to the other constructors' literals too.)
- [ ] **Step 8: Run — expect PASS.** `cargo test -p arctis-client` → all green. `cargo build -p arctis-client` confirms the constructor sweep compiles.
- [ ] **Step 9: Commit.**
  ```bash
  git add crates/engine/src/factory_profiles.rs crates/engine/src/engine.rs crates/engine/src/lib.rs crates/client/src/protocol.rs
  git commit -m "feat(protocol): FactoryProfileInfo + list/output-eq/blocksize requests"
  ```

---

### Task 10: Engine setters — `surround_set_output_eq` / `surround_set_blocksize`

**Model:** sonnet

**Files**
- Modify `crates/engine/src/engine.rs` — add two methods beside `surround_set_hrir` (lines 2605–2637) / `surround_set_channels` (lines 2640–2664).

**Interfaces**
- Produces: `Engine::surround_set_output_eq(&mut self, bands: Vec<arctis_config::EqBandConfig>) -> Result<(), EngineError>` and `Engine::surround_set_blocksize(&mut self, blocksize: Option<u32>) -> Result<(), EngineError>` — mutate the active profile's `surround`, save, re-apply when enabled (mirror `surround_set_channels`).
- Consumes: `apply_surround`, `save_config`, `config.profile_mut`.

- [ ] **Step 1: Write the failing setter tests.** In `crates/engine/src/engine.rs` tests, beside `surround_set_channels_persists_and_emits_event`:
  ```rust
  #[test]
  fn surround_set_output_eq_persists() {
      // ... build engine (surround disabled is fine — persist-only path) ...
      let bands = vec![arctis_config::EqBandConfig { kind: "peaking".into(), freq_hz: 250.0, q: 1.0, gain_db: 3.0 }];
      engine.surround_set_output_eq(bands.clone()).unwrap();
      assert_eq!(engine.config.active().unwrap().surround.output_eq, bands);
  }

  #[test]
  fn surround_set_blocksize_persists() {
      // ... build engine ...
      engine.surround_set_blocksize(Some(128)).unwrap();
      assert_eq!(engine.config.active().unwrap().surround.blocksize, Some(128));
      engine.surround_set_blocksize(None).unwrap();
      assert_eq!(engine.config.active().unwrap().surround.blocksize, None);
  }
  ```
- [ ] **Step 2: Run — expect FAIL.** `cargo test -p arctis-engine surround_set_output_eq_persists` → FAIL (no method).
- [ ] **Step 3: Implement the setters.** After `surround_set_channels` (line 2664):
  ```rust
      /// Set the explicit post-convolution surround EQ. Persists; re-applies when enabled.
      pub fn surround_set_output_eq(
          &mut self,
          bands: Vec<arctis_config::EqBandConfig>,
      ) -> Result<(), crate::error::EngineError> {
          {
              let name = self.config.active_profile.clone();
              let profile = self.config.profile_mut(&name).ok_or_else(|| {
                  crate::error::EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
              })?;
              profile.surround.output_eq = bands;
          }
          self.save_config()?;
          if self.config.active()?.surround.enabled {
              let profile = self.config.active()?.clone();
              self.apply_surround(&profile)?;
          }
          Ok(())
      }

      /// Pin (or clear) the convolver blocksize. Persists; re-applies when enabled.
      pub fn surround_set_blocksize(
          &mut self,
          blocksize: Option<u32>,
      ) -> Result<(), crate::error::EngineError> {
          {
              let name = self.config.active_profile.clone();
              let profile = self.config.profile_mut(&name).ok_or_else(|| {
                  crate::error::EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
              })?;
              profile.surround.blocksize = blocksize;
          }
          self.save_config()?;
          if self.config.active()?.surround.enabled {
              let profile = self.config.active()?.clone();
              self.apply_surround(&profile)?;
          }
          Ok(())
      }
  ```
- [ ] **Step 4: Run — expect PASS.** `cargo test -p arctis-engine surround_set_output_eq_persists surround_set_blocksize_persists` → PASS; `cargo test -p arctis-engine` → green.
- [ ] **Step 5: Commit.**
  ```bash
  git add crates/engine/src/engine.rs
  git commit -m "feat(engine): surround_set_output_eq + surround_set_blocksize setters"
  ```

---

### Task 11: Daemon dispatch + Tauri commands

**Model:** sonnet

**Files**
- Modify `crates/cli/src/daemon.rs` — add dispatch arms for the three new requests (beside `Request::SurroundSetHrir`, line 173).
- Modify `src-tauri/src/commands.rs` — add `call_factory_profiles` helper (mirror `call_streams`, lines 53–69), three `#[tauri::command]` wrappers (beside `surround_set_hrir`, line 270), and register them in `tauri::generate_handler!` (find the macro invocation in `src-tauri/src/main.rs` or `lib.rs`).

**Interfaces**
- Consumes: `Engine::list_factory_profiles`, `Engine::surround_set_output_eq`, `Engine::surround_set_blocksize`; `Request::{ListFactoryProfiles, SurroundSetOutputEq, SurroundSetBlocksize}`; `Response::ok_with_factory_profiles`.
- Produces: Tauri commands `list_factory_profiles`, `surround_set_output_eq`, `surround_set_blocksize`.

- [ ] **Step 1: Write the failing daemon dispatch test (if the daemon has request-handling unit tests).** `grep -n "fn handle\|Request::SurroundSetHrir" crates/cli/src/daemon.rs` to find the dispatch fn signature and any existing test harness. If `daemon.rs` has dispatch unit tests, add one asserting `ListFactoryProfiles` yields a `Response` with `factory_profiles: Some(..)` containing "DayZ". If there is no unit-test harness for dispatch (it drives a live engine), skip to Step 3 and rely on the `cargo build` + frontend integration; note this in the commit. Prefer adding a test if a harness exists.
- [ ] **Step 2: Run — expect FAIL** (if a test was added): `cargo test -p arctis-cli list_factory_profiles` → FAIL (non-exhaustive match / unknown variant).
- [ ] **Step 3: Add the daemon dispatch arms.** In `crates/cli/src/daemon.rs`, beside the `Request::SurroundSetHrir` arm (line 173):
  ```rust
          Request::ListFactoryProfiles => Response::ok_with_factory_profiles(engine.list_factory_profiles()),
          Request::SurroundSetOutputEq { bands } => {
              let cfg_bands: Vec<arctis_config::EqBandConfig> = bands
                  .into_iter()
                  .map(|b| arctis_config::EqBandConfig { kind: b.kind, freq_hz: b.freq_hz, q: b.q, gain_db: b.gain_db })
                  .collect();
              match engine.surround_set_output_eq(cfg_bands) {
                  Ok(()) => Response::ok_with_state(engine.state()),
                  Err(e) => Response::err(e.to_string()),
              }
          }
          Request::SurroundSetBlocksize { blocksize } => match engine.surround_set_blocksize(blocksize) {
              Ok(()) => Response::ok_with_state(engine.state()),
              Err(e) => Response::err(e.to_string()),
          },
  ```
  (Confirm `arctis_config` is in scope in `daemon.rs`; if not, add `use arctis_config;` or fully-qualify. Also confirm `engine` is the binding name used by the surrounding arms.)
- [ ] **Step 4: Add the Tauri commands.** In `src-tauri/src/commands.rs`, add a payload helper mirroring `call_streams`:
  ```rust
  async fn call_factory_profiles(
      state: &State<'_, Mutex<DaemonState>>,
      req: Request,
  ) -> Result<Vec<arctis_engine::FactoryProfileInfo>, CommandError> {
      let socket = state.lock().await.socket.clone();
      let resp = tauri::async_runtime::spawn_blocking(move || send_request_to(&socket, &req))
          .await
          .map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))??;
      if resp.ok {
          Ok(resp.factory_profiles.unwrap_or_default())
      } else {
          Err(CommandError::Daemon(resp.error.unwrap_or_else(|| "unknown daemon error".into())))
      }
  }
  ```
  Add `FactoryProfileInfo` to the `use arctis_engine::{...}` import at the top (line 4). Then the three commands (beside `surround_set_hrir`, line 270):
  ```rust
  #[tauri::command]
  pub async fn list_factory_profiles(
      state: State<'_, Mutex<DaemonState>>,
  ) -> Result<Vec<arctis_engine::FactoryProfileInfo>, CommandError> {
      call_factory_profiles(&state, Request::ListFactoryProfiles).await
  }

  #[tauri::command]
  pub async fn surround_set_output_eq(
      bands: Vec<arctis_engine::EqBandSnapshot>,
      state: State<'_, Mutex<DaemonState>>,
  ) -> Result<EngineState, CommandError> {
      call(&state, Request::SurroundSetOutputEq { bands }).await
  }

  #[tauri::command]
  pub async fn surround_set_blocksize(
      blocksize: Option<u32>,
      state: State<'_, Mutex<DaemonState>>,
  ) -> Result<EngineState, CommandError> {
      call(&state, Request::SurroundSetBlocksize { blocksize }).await
  }
  ```
  (Add `EqBandSnapshot` to the `use arctis_engine::{...}` import.)
- [ ] **Step 5: Register the commands.** `grep -rn "surround_set_hrir" src-tauri/src/*.rs` to find the `tauri::generate_handler![ ... ]` list; add `commands::list_factory_profiles, commands::surround_set_output_eq, commands::surround_set_blocksize,` beside `commands::surround_set_hrir`.
- [ ] **Step 6: Run — expect PASS / build green.** `cargo test -p arctis-cli` (if a dispatch test was added) → PASS; `cargo build -p arctis-cli -p arctis-tauri` (use the real `src-tauri` package name from `src-tauri/Cargo.toml`) → compiles, match is exhaustive.
- [ ] **Step 7: Commit.**
  ```bash
  git add crates/cli/src/daemon.rs src-tauri/src/commands.rs src-tauri/src/main.rs
  git commit -m "feat(tauri): list_factory_profiles + surround output-eq/blocksize commands"
  ```

---

### Task 12: Frontend IPC bindings + TS types + pure surround helpers

**Model:** sonnet

**Files**
- Modify `frontend/src/lib/ipc.ts` — `SurroundSnapshot` interface (lines 83–97); add `FactoryProfileInfo` interface; add `listFactoryProfiles`, `surroundSetOutputEq`, `surroundSetBlocksize` (beside `surroundSetHrir`, lines 343–353).
- Modify `frontend/src/lib/surround.ts` — add pure helpers: `groupHrirOptionsByTonality`, `outputEqToBands`, `bandsToOutputEq`, `factoryProfileLabel`.
- Modify `frontend/src/lib/surround.test.ts` — vitest for the new pure helpers (NO jsdom).

**Interfaces**
- Produces (TS): `SurroundSnapshot.hrir_missing?: string | null`, `.output_eq?: EqBandSnapshot[]`, `.blocksize?: number | null`; `FactoryProfileInfo { name; hrir: string | null; mode: string }`; `listFactoryProfiles(): Promise<FactoryProfileInfo[]>`, `surroundSetOutputEq(bands: EqBandSnapshot[]): Promise<EngineState>`, `surroundSetBlocksize(blocksize: number | null): Promise<EngineState>`.
- Produces (pure): `groupHrirOptionsByTonality(entries) -> SelectOption[]` (grouped Dry/Neutral/Roomy via existing `tonality`), `outputEqToBands(snap: EqBandSnapshot[]) -> Band[]`, `bandsToOutputEq(bands: Band[]) -> EqBandSnapshot[]`, `factoryProfileLabel(info) -> string`.
- Consumes: `EqBandSnapshot` (already in `ipc.ts`), `HrirEntrySnapshot`, `Band` (from `eq.ts`), `SelectOption`.

- [ ] **Step 1: Write the failing pure-helper tests.** In `frontend/src/lib/surround.test.ts`, append (and extend the import line to include the new helpers):
  ```ts
  import { groupHrirOptionsByTonality, outputEqToBands, bandsToOutputEq, factoryProfileLabel } from "./surround.js";
  import type { HrirEntrySnapshot, EqBandSnapshot, FactoryProfileInfo } from "./ipc.js";

  describe("groupHrirOptionsByTonality", () => {
    const entries: HrirEntrySnapshot[] = [
      { stem: "a", display: "A", group: "G", tonality: "Roomy" },
      { stem: "b", display: "B", group: "G", tonality: "Dry" },
      { stem: "c", display: "C", group: "G", tonality: "Neutral" },
    ];
    it("orders Dry, then Neutral, then Roomy", () => {
      const opts = groupHrirOptionsByTonality(entries);
      const stems = opts.map((o) => o.value);
      expect(stems).toEqual(["b", "c", "a"]);
    });
    it("falls back to display when group is empty", () => {
      const opts = groupHrirOptionsByTonality([{ stem: "x", display: "X", group: "", tonality: "Dry" }]);
      expect(opts[0].label).toBe("X");
    });
  });

  describe("outputEqToBands / bandsToOutputEq round-trip", () => {
    const snap: EqBandSnapshot[] = [{ kind: "peaking", freq_hz: 250, q: 1, gain_db: 3 }];
    it("maps snake_case to camelCase and back", () => {
      const bands = outputEqToBands(snap);
      expect(bands[0]).toEqual({ kind: "peaking", freqHz: 250, q: 1, gainDb: 3 });
      expect(bandsToOutputEq(bands)).toEqual(snap);
    });
  });

  describe("factoryProfileLabel", () => {
    it("shows name and hrir", () => {
      const info: FactoryProfileInfo = { name: "DayZ", hrir: "04-gsx-sennheiser-gsx", mode: "hrir71" };
      expect(factoryProfileLabel(info)).toContain("DayZ");
    });
  });
  ```
- [ ] **Step 2: Run — expect FAIL.** `cd frontend && npx vitest run src/lib/surround.test.ts` → FAIL (helpers not exported).
- [ ] **Step 3: Implement the TS types.** In `frontend/src/lib/ipc.ts`, extend `SurroundSnapshot` (after `negotiated_channels`, line 96):
  ```ts
    /** Pinned HRIR stem requested but not installed (fallback in use); null/absent when OK. */
    hrir_missing?: string | null;
    /** Explicit post-convolution EQ on the binaural tail (empty/absent = none). */
    output_eq?: EqBandSnapshot[];
    /** Pinned convolver blocksize, or null/absent for PipeWire default. */
    blocksize?: number | null;
  ```
  Add after `MicPresetSnapshot` (line 107):
  ```ts
  /** Mirror of crates/engine/src/factory_profiles.rs FactoryProfileInfo. */
  export interface FactoryProfileInfo {
    name: string;
    hrir: string | null;
    mode: string;
  }
  ```
- [ ] **Step 4: Implement the IPC functions.** In `frontend/src/lib/ipc.ts`, beside `surroundSetHrir` (after line 353):
  ```ts
  /** List the static factory-profile catalog for the data-driven create-profile UI. */
  export const listFactoryProfiles = (): Promise<FactoryProfileInfo[]> =>
    invoke<FactoryProfileInfo[]>("list_factory_profiles");

  /** Set the explicit post-convolution surround EQ bands. Returns updated EngineState. */
  export const surroundSetOutputEq = (bands: EqBandSnapshot[]): Promise<EngineState> =>
    invoke<EngineState>("surround_set_output_eq", { bands });

  /** Pin (or clear) the convolver blocksize. null = PipeWire default. */
  export const surroundSetBlocksize = (blocksize: number | null): Promise<EngineState> =>
    invoke<EngineState>("surround_set_blocksize", { blocksize });
  ```
- [ ] **Step 5: Implement the pure helpers.** In `frontend/src/lib/surround.ts`, add (importing `Band` from `./eq.js`, `HrirEntrySnapshot`/`EqBandSnapshot`/`FactoryProfileInfo` from `./ipc.js`):
  ```ts
  import type { HrirEntrySnapshot, EqBandSnapshot, FactoryProfileInfo } from "./ipc.js";
  import type { Band } from "./eq.js";

  const TONALITY_ORDER = ["Dry", "Neutral", "Roomy"] as const;

  /** Group HRIR options by tonality (Dry → Neutral → Roomy), stable within each group. */
  export function groupHrirOptionsByTonality(entries: HrirEntrySnapshot[]): SelectOption[] {
    const rank = (t: string): number => {
      const i = TONALITY_ORDER.indexOf(t as (typeof TONALITY_ORDER)[number]);
      return i === -1 ? TONALITY_ORDER.length : i;
    };
    return [...entries]
      .sort((a, b) => rank(a.tonality) - rank(b.tonality))
      .map((e) => ({
        value: e.stem,
        label: e.group ? `${e.group} — ${e.display}` : e.display,
      }));
  }

  /** Map engine output_eq snapshot bands to editor Band[] (snake → camel). */
  export function outputEqToBands(snap: EqBandSnapshot[]): Band[] {
    return snap.map((b) => ({ kind: b.kind as Band["kind"], freqHz: b.freq_hz, q: b.q, gainDb: b.gain_db }));
  }

  /** Inverse of outputEqToBands: editor Band[] → engine output_eq snapshot bands. */
  export function bandsToOutputEq(bands: Band[]): EqBandSnapshot[] {
    return bands.map((b) => ({ kind: b.kind, freq_hz: b.freqHz, q: b.q, gain_db: b.gainDb }));
  }

  /** Human label for a factory-profile row. */
  export function factoryProfileLabel(info: FactoryProfileInfo): string {
    return info.hrir ? `${info.name} · ${info.hrir}` : info.name;
  }
  ```
- [ ] **Step 6: Run — expect PASS.** `cd frontend && npx vitest run src/lib/surround.test.ts` → PASS; `cd frontend && npm run check` → no type errors.
- [ ] **Step 7: Commit.**
  ```bash
  git add frontend/src/lib/ipc.ts frontend/src/lib/surround.ts frontend/src/lib/surround.test.ts
  git commit -m "feat(ui): IPC + pure helpers for factory list, output EQ, blocksize, tonality grouping"
  ```

---

### Task 13: SpatialPage — data-driven factory list, missing-HRIR prompt, post EQ, tonality grouping

**Model:** sonnet

**Files**
- Modify `frontend/src/lib/components/SpatialPage.svelte` — `<script>` imports (lines 20–45), HRIR options (lines 64–76), handlers (lines 109–167), the FACTORY PROFILES control-row (lines 224–252), and add two new card sections in the template.

> **Deviation note (verified against code):** the hardcoded factory button lives in **`SpatialPage.svelte`** (the FACTORY PROFILES `control-row`, lines 224–252 + `onCreateDayZ`, lines 156–167), **not** in `ProfilesDropdown.svelte` — `ProfilesDropdown.svelte` contains no "Create DayZ profile" button (`grep "DayZ" frontend/src/lib/components/ProfilesDropdown.svelte` → no matches). The spec's intent (§6.3) is "replace the hardcoded button with a data-driven list", so this task does that **in SpatialPage where the button actually is**. `ProfilesDropdown.svelte` is left unchanged.

**Interfaces**
- Consumes: `listFactoryProfiles`, `surroundSetOutputEq`, `profileCreateFromFactory`, `groupHrirOptionsByTonality`, `outputEqToBands`, `bandsToOutputEq`, `factoryProfileLabel`, `EqEditor` (props `bands`, `selectedIndex`, `onBandChange`, `onFlush`).
- Produces: thin view wiring only; all non-trivial logic stays in `surround.ts` (tested in Task 12).

- [ ] **Step 1: Confirm test coverage is in `surround.ts`.** The pure logic (grouping, band mapping, factory label) is already tested in Task 12. SpatialPage stays a thin view — no jsdom test. Verify by reading the current `<script>` (lines 1–168) before editing.
- [ ] **Step 2: Wire the data-driven factory list.** In `SpatialPage.svelte` `<script>`:
  - Add imports: `listFactoryProfiles, surroundSetOutputEq` to the `ipc.js` import group (lines 22–32); `groupHrirOptionsByTonality, outputEqToBands, bandsToOutputEq, factoryProfileLabel` to the `surround.js` import group (lines 33–40); `EqEditor from "./EqEditor.svelte"`; `type FactoryProfileInfo` and `type EqBandSnapshot` from `../ipc.js`.
  - Add state + load on mount:
    ```ts
    let factoryProfiles = $state<FactoryProfileInfo[]>([]);
    onMount(() => {
      listFactoryProfiles().then((f) => (factoryProfiles = f)).catch(() => {});
    });

    async function onCreateFactory(name: string) {
      importBusy = true;
      importMsg = null;
      try {
        await profileCreateFromFactory(name).then(applyState);
        importMsg = `${name} profile created.`;
      } catch (err) {
        importMsg = String(err);
      } finally {
        importBusy = false;
      }
    }
    ```
    Remove the now-unused `onCreateDayZ` (lines 156–167).
  - Replace the FACTORY PROFILES `control-row` (lines 224–237) with a data-driven `{#each}`:
    ```svelte
            <div class="control-row">
              <span class="field-label">FACTORY PROFILES</span>
              <div class="hrir-actions">
                {#each factoryProfiles as fp (fp.name)}
                  <button
                    class="ss-btn"
                    disabled={importBusy}
                    onclick={() => onCreateFactory(fp.name)}
                    title={`Create the ${fp.name} factory profile (${factoryProfileLabel(fp)}).`}
                    aria-label={`Create ${fp.name} factory profile`}
                  >
                    {importBusy ? "Working…" : `Create ${fp.name} profile`}
                  </button>
                {/each}
              </div>
            </div>
    ```
- [ ] **Step 3: Group the HRIR Select by tonality.** Replace the `availableHrirEntries.length > 0 ? availableHrirEntries.map(...)` branch inside `hrirOptions` (lines 70–75) with a call to the pure helper:
  ```ts
      ...(availableHrirEntries.length > 0
        ? groupHrirOptionsByTonality(availableHrirEntries)
        : availableHrirs.map((stem) => ({ value: stem, label: hrirDisplayName(stem) }))),
  ```
- [ ] **Step 4: Render the missing-HRIR prompt.** Add a derived flag and a dismissible banner. In `<script>`:
  ```ts
  const hrirMissing = $derived(surround?.hrir_missing ?? null);
  let missingDismissed = $state(false);
  ```
  In the template, after the "No HRIR profiles" banner (line 194) and before the ADD HRIR card:
  ```svelte
      {#if hrirMissing && !missingDismissed}
        <div class="banner banner--warn" role="alert">
          <span class="banner-icon" aria-hidden="true">◎</span>
          <div class="banner-body">
            <span class="banner-title">{hrirMissing} not installed</span>
            <span class="banner-desc">A bundled fallback HRIR is in use. Import your HRIRs to use the pinned profile.</span>
          </div>
          <button class="ss-btn" disabled={importBusy} onclick={onImportHrirs} aria-label="Import your HRIRs">Import your HRIRs</button>
          <button class="ss-btn ss-btn--ghost" onclick={() => (missingDismissed = true)} aria-label="Dismiss">Dismiss</button>
        </div>
      {/if}
  ```
- [ ] **Step 5: Add the "Spatial correction (post)" EQ section.** Add derived state for the post-EQ bands and an enable toggle, reusing `EqEditor`:
  ```ts
  const postEqBands = $derived(outputEqToBands(surround?.output_eq ?? []));
  let postSelected = $state(0);
  const postEnabled = $derived((surround?.output_eq ?? []).length > 0);

  function onPostBandChange(index: number, band: import("../eq.js").Band) {
    const next = postEqBands.map((b, i) => (i === index ? band : b));
    surroundSetOutputEq(bandsToOutputEq(next)).then(applyState).catch((err) => {
      console.warn("[SpatialPage] surroundSetOutputEq failed:", err);
    });
  }
  function onPostToggle(on: boolean) {
    // Toggling off clears the curve; toggling on seeds a flat 10-band set.
    const bands = on ? bandsToOutputEq(outputEqToBands([])) : [];
    surroundSetOutputEq(bands).then(applyState).catch(() => {});
  }
  ```
  > Worker note: if a flat seed is wanted on enable, source the 10 canonical bands from `eq.ts` `STANDARD_FREQS` / `defaultBands()` (read `eq.ts` for the exact export) rather than an empty array, so the editor has bands to manipulate. Keep this mapping logic in `surround.ts` if it grows beyond a one-liner (add `flatOutputEq(): EqBandSnapshot[]` + a vitest). In the template, add a new `device-card` after the master-enable card with a `Switch` bound to `postEnabled`/`onPostToggle` and, when `postEnabled`, the editor:
  ```svelte
      <EqEditor bands={postEqBands} selectedIndex={postSelected}
        onBandChange={onPostBandChange} onSelect={(i) => (postSelected = i)} onFlush={onPostBandChange} />
  ```
- [ ] **Step 6: Run checks — expect PASS.** `cd frontend && npm run check` → no type errors; `cd frontend && npm test` → existing + Task 12 vitest green. (No jsdom; SpatialPage is verified via type-check + the pure helpers it delegates to.)
- [ ] **Step 7: Commit.**
  ```bash
  git add frontend/src/lib/components/SpatialPage.svelte frontend/src/lib/surround.ts frontend/src/lib/surround.test.ts
  git commit -m "feat(ui): data-driven factory list, missing-HRIR prompt, post-EQ, tonality grouping in SpatialPage"
  ```

---

### Task 14: Full-workspace + frontend verification

**Model:** haiku

**Files**
- None (verification only).

**Interfaces**
- Consumes: the entire workspace + frontend.

- [ ] **Step 1: Rust workspace tests.** `cargo test --workspace` → expect all green (config round-trips, presets well-formed + DayZ Spatial, convert eq_model_from_bands + fallback, audio blocksize snapshots, engine apply_surround + factory catalog + setters, client protocol round-trips, daemon dispatch).
- [ ] **Step 2: Rust lint (if the repo gates on it).** `cargo clippy --workspace --all-targets` → no new warnings on runtime paths (confirm no `unwrap`/`expect` outside `#[cfg(test)]`).
- [ ] **Step 3: Frontend type-check + tests.** `cd frontend && npm run check && npm test` → expect green (svelte-check passes; vitest passes including the new `surround.test.ts` cases).
- [ ] **Step 4: Manual smoke (owner, optional, requires daemon restart).** Restart `asm-cli` daemon, open the GUI, confirm: the Spatial page lists "Create DayZ profile" from the catalog; creating it pins GSX (or shows the missing-HRIR prompt with a working "Import your HRIRs" button if GSX is absent); the "Spatial correction (post)" editor shows the 10-band DayZ Spatial curve. This is the owner's in-game acceptance step (spec §9) and is **not** a blocking automated gate.
- [ ] **Step 5: Commit (if Steps 1–3 required any fixups).**
  ```bash
  git add -A
  git commit -m "test: workspace + frontend green for DayZ surround profile + factory catalog"
  ```

---

## Spec coverage check (self-review)

- §3 EQ structure (single combined post EQ in `SurroundConfig.output_eq`) → Tasks 1, 2, 6.
- §3 DayZ HRIR pin GSX + fallback + import prompt → Tasks 3, 6, 13.
- §3 blocksize 128 → Tasks 1, 4, 6, 7.
- §3 full scaffolding (catalog, generic apply, Tauri/UI listing, two new fields) → Tasks 1, 7, 8, 9, 11, 13.
- §4 A1 catalog (`ChannelEqSeed`, `FactoryProfileSpec`, `factory_profiles`, `find_factory_profile`, `apply_factory_spec`) → Task 7; `create_factory_profile` catalog-driven → Task 8.
- §4 A2 explicit `output_eq` preferred, legacy relocation fallback, flatten unchanged → Task 6.
- §4 A3 additive `#[serde(default)]` fields, old TOML identical → Task 1.
- §4 A4 missing-HRIR fallback + `hrir_missing` flag in state, UI prompt → Tasks 3, 5, 6, 13.
- §4 A5 tonality grouping (UI-only, no catalog field) → Tasks 12, 13.
- §5 DAYZ entry exact values + "DayZ Spatial" 10-band curve → Tasks 2, 7.
- §6.1 renderer blocksize emission + `None` snapshot identical + `Some(128)` test → Task 4.
- §6.2 engine apply path (output_eq source, blocksize thread, fallback flag, catalog) → Tasks 6, 8.
- §6.3 Tauri `list_factory_profiles`, data-driven button, post-EQ section, tonality grouping, `surroundSetOutputEq`/`surroundSetBlocksize` → Tasks 9, 11, 12, 13.
- §6.4 back-compat: no destructive migration; existing saved DayZ not rewritten → Tasks 1, 5 (serde defaults).
- §7 testing strategy → covered task-by-task; frontend pure-helpers only (no jsdom) → Task 12.
- §8 out-of-scope items (more profiles, head-tracking, blocksize policy, mic profile) → intentionally not built; the catalog is structured so each future game is one struct literal.
