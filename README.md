# Astro-Telemetry: Simulated Space Bio-Telemetry Across Two Windows PCs

This prototype runs on two PCs. **Computer 1** pretends to be a chip implanted in an
astronaut and sends vital signs over Bluetooth Low Energy. **Computer 2** is Ground
Control: it receives the readings, has a Python engine analyse them, and shows a
live dashboard.

```
 COMPUTER 1 (bio-chip-sim, Rust)                 COMPUTER 2 (ground-control)
 ┌───────────────────────────────┐   BLE GATT    ┌────────────────────────────────────────┐
 │ physiology model (1 Hz)       │  notify, 18 B │ source.rs   btleplug central (tokio)    │
 │  HR · SpO2 · Temp · BP · Rad  │ ────────────► │   │ RawTelemetry                         │
 │ WinRT GattServiceProvider     │               │   ▼                                      │
 │ TCP JSONL server (loopback)   │ ── or TCP ──► │ bridge.rs ── stdin JSONL ──► analysis.py │
 └───────────────────────────────┘               │           ◄─ stdout JSONL ── (Python)    │
                                                 │   │ Enriched                             │
                                                 │   ▼                                      │
                                                 │ app.rs  eframe/egui + egui_plot          │
                                                 │ export.rs  ──► exports/session_*.csv     │
                                                 └────────────────────────────────────────┘
```

It covers four spaceflight hazard groups (space radiation, isolation, altered gravity, hostile closed
environment) with **19 monitored features**. See §10 for the full list, signals and alert rules.

Which language does what:
- **Rust** handles BLE (both the peripheral and the central), process management, the UI, the charts and CSV export.
- **Python** is used only for analysis and anomaly detection, in `ground-control/python/analysis.py`. It uses the standard library only, so there is nothing to `pip install`.

---

## 1. Project structure

```
astro-telemetry/
├── Cargo.toml                    # workspace (3 crates)
├── README.md
├── setup.bat / setup.ps1         # dependency checker + installer
├── scripts/
│   ├── run-computer1-ble.ps1     # Computer 1 launcher
│   ├── run-computer2-ble.ps1     # Computer 2 launcher
│   └── run-single-pc-loopback.ps1
├── telemetry-protocol/           # SHARED by both PCs
│   ├── Cargo.toml
│   └── src/lib.rs                # GATT UUIDs, RawTelemetry, 18-byte core + 19-byte extended frames
├── bio-chip-sim/                 # ── COMPUTER 1 ──
│   ├── Cargo.toml                # windows 0.58 (WinRT BLE peripheral), clap, rand
│   └── src/
│       ├── main.rs               # CLI, 1 Hz loop, console event injection
│       ├── physiology.rs         # Ornstein-Uhlenbeck core vitals model + scenario events
│       ├── extended.rs           # slow/environment signals: immune, bone, muscle, sleep, cabin air...
│       ├── ble.rs                # GattServiceProvider: advertise, Read + Notify
│       └── tcp.rs                # Local Mock/Loopback JSON-lines server
└── ground-control/               # ── COMPUTER 2 ──
    ├── Cargo.toml                # eframe/egui_plot 0.29, btleplug 0.11, tokio, csv
    ├── python/
    │   ├── analysis.py           # core vitals: rolling stats, thresholds, z-scores, dosimetry
    │   └── ext_analysis.py       # the 19 hazard features, baselines, risk scores, hysteresis
    ├── testdata/engine_output_sample.jsonl   # real engine output used by a Rust contract test
    └── src/
        ├── main.rs               # CLI + wiring of threads/channels
        ├── model.rs              # shared types (Enriched, Level, AppEvent…)
        ├── source.rs             # BLE central (btleplug) / TCP client, auto-reconnect
        ├── bridge.rs             # Python child-process supervisor + JSONL IPC
        ├── app.rs                # egui dashboard
        └── export.rs             # CSV export
```

**Copy the whole `astro-telemetry` folder to both PCs.** Each PC builds only the crate it needs.

---

## 2. Install the prerequisites (on both PCs)

**Easiest:** double-click `setup.bat` (or run it in a terminal). It checks Windows, the Bluetooth radio,
the Visual Studio C++ Build Tools, Rust (MSVC, 1.80+), Python (3.8+, Computer 2 only) and internet access,
then installs whatever is missing after asking (winget if you have it, otherwise a direct download from Microsoft, rustup.rs or python.org).
It never touches tools that are already present. Everything it prints is also saved to `setup.log`.

```powershell
.\setup.bat                           # check + install everything
.\setup.bat -Role computer1 -Build    # Computer 1 only (no Python), then run tests and compile
.\setup.bat -Role computer2 -Build    # Computer 2 only
.\setup.bat -CheckOnly                # report only, install nothing
.\setup.bat -Yes                      # install without asking
```

A Windows UAC prompt appears for the C++ Build Tools (5-20 minutes). If it says "open a NEW terminal", do that and run it again.
Extract the zip first: running `setup.bat` from inside the zip does not work. If setup still fails, send me `setup.log`.
The manual commands below do the same thing.


Run these in **PowerShell**:

```powershell
# Rust (MSVC toolchain) + the C++ build tools it links with
winget install --id Rustlang.Rustup -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"

# Python 3 (only needed on Computer 2)
winget install --id Python.Python.3.12 -e
```

**Close PowerShell and open a new window** so the PATH updates take effect. Then:

```powershell
rustup default stable-msvc
rustc --version          # 1.80 or newer
python --version         # or:  py -3 --version
```

> If typing `python` opens the Microsoft Store, Ground Control falls back to `py -3`
> on its own. You can also turn off the alias in *Settings → Apps → Advanced app
> settings → App execution aliases*.

Check the Python engine on its own (Computer 2):

```powershell
cd astro-telemetry
python ground-control\python\analysis.py --selftest     # should print: SELFTEST PASSED
```

Check the Rust frame codec and the physiology model (either PC):

```powershell
cargo test -p telemetry-protocol -p bio-chip-sim
```

---

## 3. Bluetooth setup and connection

You do **not** need to pair the PCs in Windows Settings. The bio-chip exposes an
open GATT service (`GattProtectionLevel::Plain`), and Ground Control finds it by
its custom service UUID `7a3e0001-5c2d-4f8b-9e61-b0c4d5a2e7f3` and connects
directly.

1. On **both PCs**, turn Bluetooth on: *Settings → Bluetooth & devices → Bluetooth: On*.
2. On **Computer 1**, the radio must support the BLE **peripheral role**. The simulator checks this when it starts and prints:
   `[BLE] Adapter: LE supported = true, peripheral role supported = true`.
   Most Intel AX2xx and recent Realtek/MediaTek radios support it. Some older USB dongles do not.
3. Keep the PCs within about 5 m of each other for the first test.
4. If the PCs were previously paired as *classic* Bluetooth devices and the connection behaves oddly, remove that pairing. Stale pairings can confuse the GATT cache.

---

## 4. Run it on two PCs over BLE

**Computer 1: bio-chip simulator**

```powershell
cd astro-telemetry
cargo run --release -p bio-chip-sim -- --mode ble
# or: .\scripts\run-computer1-ble.ps1
```

Expected output: `Advertising service 7a3e0001-…`, and then one telemetry line every second.
When Ground Control connects you'll see `Ground Control subscribed (1 client(s))`.

**Computer 2: Ground Control**

```powershell
cd astro-telemetry
cargo run --release -p ground-control -- --source ble
# or: .\scripts\run-computer2-ble.ps1
```

The status pills move from `LINK: SCANNING` → `CONNECTING` → `LINK: BLE`, and `PYTHON: ON`.
The vitals cards and charts then start updating.

If the scan doesn't find the device, Computer 1's adapter may be putting the UUID only in the scan response. Match on Computer 1's Bluetooth name instead:

```powershell
cargo run --release -p ground-control -- --source ble --device-name "DESKTOP-ABC123"
```

---

## 5. Run it on one PC (Local Mock/Loopback mode)

A single PC can't connect to its own BLE peripheral, so loopback mode uses TCP instead, with the same data model:

```powershell
# Terminal 1
cargo run --release -p bio-chip-sim -- --mode tcp
# Terminal 2
cargo run --release -p ground-control -- --source tcp
# or, both in one go:
.\scripts\run-single-pc-loopback.ps1
```

**LAN fallback** (two PCs, no BLE-capable hardware):

```powershell
# Computer 1 (allow the Windows Firewall prompt)
cargo run --release -p bio-chip-sim -- --mode tcp --bind 0.0.0.0:7878
# Computer 2
cargo run --release -p ground-control -- --source tcp --addr 192.168.1.50:7878
```

`--mode both` broadcasts over BLE and TCP at the same time.

---

## 6. Demo: triggering events

In the **simulator** console, type a letter and press Enter:

| key | event | what Ground Control should show |
|---|---|---|
| `t` | Tachycardia (EVA stress) | HR ≈ 146 → TACHYCARDIA (CRITICAL), BP rises |
| `b` | Bradycardia | HR ≈ 42 → BRADYCARDIA |
| `h` | Hypoxia (cabin O₂ drop) | SpO₂ ≈ 86 % → HYPOXIA (CRITICAL) |
| `r` | Solar particle event | dose rate ≈ 380 µSv/h → RADIATION_EXPOSURE (CRITICAL) |
| `f` | Fever | 38.9 °C → HYPERTHERMIA (WARNING) |
| `p` | Hypertensive crisis | 184/119 → HYPERTENSION (CRITICAL) |
| `g` | Sensor glitch (1 sample) | RAPID_CHANGE only. The persistence filter suppresses a false alarm |
| `c` | Cabin CO₂ build-up | ppCO₂ → ~7 mmHg → HIGH_CO2, reaction time and respiratory rate rise |
| `i` | Infection | microbial load, CRP, mild fever; lymphocytes fall → infection-risk, immune alerts |
| `s` | Sleep loss (several nights) | SHORT_SLEEP, SLEEP_DEBT, then MOOD_DECLINE |
| `x` | Skip exercise countermeasures | COUNTERMEASURE_SKIPPED, then BONE_LOSS_RATE, MUSCLE_LOSS, ORTHOSTATIC_INTOLERANCE |
| `m` | Low-mood episode | MOOD_DECLINE / LOW_MOOD |
| `d` | Poor nutrition (low calcium and calories, ~12 days) | CALCIUM_LOW, ENERGY_DEFICIT, VITAMIN_D_LOW, faster bone-turnover rise |
| `w` | Workload surge (~3 days) | WORKLOAD_HIGH, LONG_WORK_DAY, then SHORT_SLEEP and MOOD_DECLINE |
| `o` | Communication outage (~4 days) | EARTH_COMMS_LOW, SOCIAL_ISOLATION, then MOOD_DECLINE |
| `v` | Reporting silence (~5 days) | SELF_MONITORING_LAPSE, REPORT_DELAY, PEER_CHECK_LAPSE |
| `n` | Back to nominal | alerts clear and are logged as *Cleared* |

`r` (solar particle event) also slows reaction time (CNS) and depresses lymphocytes; `h` (hypoxia) also drops cabin O₂.
Random events also happen on their own (`--event-rate 0.012` per second by default). Use `--no-random-events` for a scripted demo, or `--seed 42` to make a run reproducible.

Click **Export Session (.csv)** to write `exports\session_YYYYmmdd_HHMMSS.csv`. Each sample gets one row containing: the raw vitals, the rolling averages, the status of each vital, the overall status, the anomaly score, the cumulative dose and the alert text.

---

## 7. Interfaces

**BLE frames** (both fit the minimum 20-byte ATT payload; see `telemetry-protocol/src/lib.rs`):
- *Core characteristic*, 18 bytes: version, sequence, HR×10, SpO₂×10, Temp×100, systolic, diastolic, Rad×10, XOR checksum.
- *Extended characteristic*, 19 bytes: version, group (0-5), sequence, six signed 16-bit values, XOR checksum. Groups 0-1 (cabin air, HRV, immune, reaction time...) go out every second; groups 2-5 (sleep, mood, bone, muscle, fluids, CRP, nutrition, workload, social contact, reporting...) every `--slow-every` seconds. Ground Control keeps the last value of each field, and a group's `groups_mask` bit says whether it has arrived yet.
- Older simulators without the extended characteristic still work: the dashboard shows core vitals and "NO DATA" on the Risk tab.

**Rust → Python** (stdin): one JSON line per sample:

```json
{"seq":12,"heart_rate_bpm":72.3,"spo2_pct":98.1,"core_temp_c":36.92,"bp_systolic_mmhg":118.0,"bp_diastolic_mmhg":76.0,"radiation_usv_h":18.4,"received_at_ms":1731600000000}
```

**Python → Rust** (stdout): `{"type":"telemetry", "raw":{…}, "avg":{…}, "trend_per_min":{…}, "zscore":{…}, "status":{…}, "overall":"WARNING", "alerts":[{"vital","level","code","message"}], "cumulative_dose_usv", "projected_daily_dose_msv", "anomaly_score", …}`.
Python's stderr appears in the dashboard's *System log*. If Python crashes, it restarts automatically.

**Analysis rules** (editable in `LIMITS` in `analysis.py`):

| vital | warning | critical |
|---|---|---|
| Heart rate | < 50 or > 100 bpm | < 40 or > 130 bpm |
| SpO₂ | < 94 % | < 90 % |
| Core temperature | < 36.0 or > 38.0 °C | < 35.0 or > 39.5 °C |
| Systolic / diastolic BP | > 140 / > 90 or < 90 / < 50 | > 180 / > 120 or systolic < 80 |
| Radiation dose rate | > 50 µSv/h | > 200 µSv/h |

- A WARNING must hold for `--persistence` consecutive samples (default 2). A CRITICAL raises an alarm immediately.
- RAPID_CHANGE fires when a sample's z-score against the rolling window is at least `--z-threshold` (default 4.5).

---

## 8. Useful flags

```text
bio-chip-sim   --mode ble|tcp|both  --bind 127.0.0.1:7878  --interval-ms 1000
               --event-rate 0.012   --no-random-events     --seed N   --print-every 1
               --time-scale 3600    --start-day 1          --slow-every 5
ground-control --source ble|tcp     --addr 127.0.0.1:7878  --device-name TEXT
               --scan-timeout 30    --python PATH          --script PATH
               --window 30          --persistence 2        --z-threshold 4.5
               --career-limit-msv 600
               --export-dir exports
```

---

## 9. Troubleshooting

| symptom | fix |
|---|---|
| `peripheral role supported = false` | Update the Bluetooth driver or use a capable dongle. Otherwise use the LAN fallback (§5). |
| `GattServiceProvider … RadioNotAvailable` | Bluetooth is off, or another app holds the radio. Toggle Bluetooth and retry. |
| Ground Control stays on SCANNING | Make sure the simulator shows `Advertisement status: Started`. Then try `--device-name`, and move the PCs closer. |
| Connects, then `LINK: LOST` loops | Remove old pairings between the PCs and restart both apps. Windows sometimes caches stale GATT tables. |
| `PYTHON: DOWN` | Hover the pill to see why. Install Python 3.8+ or pass `--python C:\Path\to\python.exe`. |
| No data in loopback mode | Start the simulator with `--mode tcp` *before* Ground Control. It also retries every 3 s on its own. |
| Build error mentioning `link.exe` | The VS C++ Build Tools are missing. See §2. |

---

## 10. Hazard coverage: the 19 monitored features

**Time acceleration.** Slow effects need time, so the simulator runs a *mission clock*: `--time-scale 3600`
(default) means 1 real second = 1 mission hour and one mission day passes in 24 s. Use `--time-scale 1` for real time.
Dose accumulates in mission time, and the engine integrates it with the same scale.

**Dashboard.** The **Risk assessment** tab shows a composite mission-risk score, four hazard sections with a risk bar per feature
(0-100: below 30 nominal, 30-65 warning, above 65 critical), and long-term charts against mission day. Alerts also appear
in the right-hand panel and in the CSV export.

| Hazard | Feature | Simulated signal(s) | What `ext_analysis.py` does | Alert codes |
|---|---|---|---|---|
| Space radiation | Immune | lymphocyte count | decline vs personal baseline, absolute lymphopenia, dose-response slope | IMMUNE_SUPPRESSION, LYMPHOPENIA |
| | Cardiovascular | HRV (RMSSD), pulse-wave velocity | 2-minute HRV mean vs baseline, arterial stiffening vs baseline | HRV_SUPPRESSION, ARTERIAL_STIFFENING |
| | Central nervous system | reaction time, error rate | median reaction time vs baseline, errors, flags elevated dose-rate | CNS_SLOWING, ERROR_RATE |
| | Cancer risk | cumulative effective dose | linear no-threshold estimate (5.5 %/Sv, illustrative), % of career limit, days to limit | DOSE_LIMIT |
| Isolation and behavioral health | Sleep | sleep hours, efficiency | last night, 7-night debt, efficiency | SHORT_SLEEP, SLEEP_DEBT, POOR_SLEEP_QUALITY |
| | Mood | daily mood score | drop vs first-3-day baseline, absolute level | MOOD_DECLINE, LOW_MOOD |
| | Cognition | reaction time, errors | 3-day mean vs baseline, ms/day trend over 7 days | COGNITIVE_DECLINE, COGNITIVE_DRIFT, COGNITIVE_ERRORS |
| | Workload | workload index, work-day length | 3-day median vs limits | WORKLOAD_HIGH, LONG_WORK_DAY |
| | Confinement | social contact minutes, Earth-communication minutes | 3-day median vs minimum contact | SOCIAL_ISOLATION, EARTH_COMMS_LOW |
| | Self-reporting | self-check completion, peer checks, symptom-report delay | 3-day median vs limits | SELF_MONITORING_LAPSE, PEER_CHECK_LAPSE, REPORT_DELAY |
| Altered gravity | Bone density | bone-turnover marker (NTx) | projected %/month loss, cumulative estimate | BONE_LOSS_RATE, BONE_LOSS_CUM |
| | Muscle mass | lean mass, leg strength, exercise minutes | weekly loss rate, cumulative loss, strength | MUSCLE_LOSS_RATE, MUSCLE_LOSS_CUM, STRENGTH_LOSS |
| | Fluid shifts | bioimpedance fluid index | excess over the expected adaptation curve | FLUID_SHIFT |
| | CV deconditioning | orthostatic (stand-test) HR rise | rise vs limits, EVA/landing readiness label | ORTHOSTATIC_INTOLERANCE |
| | Mechanical loading | exercise minutes, loading as % of prescription | 3-day median vs prescription | COUNTERMEASURE_SKIPPED, LOADING_DEFICIT |
| | Nutrition | calcium, energy, vitamin D, sodium | per-nutrient limits; poor nutrition also speeds bone loss in the simulator | CALCIUM_LOW, ENERGY_DEFICIT, VITAMIN_D_LOW, SODIUM_HIGH |
| Closed environment | Immune function | lymphocytes, cortisol, CRP | composite immune index 0-100 | IMMUNE_DEPRESSION |
| | Respiratory health | CO₂, O₂, PM2.5, humidity, respiratory rate | per-parameter limits | HIGH_CO2, LOW_O2, PARTICULATES, ABNORMAL_RESP_RATE, HUMIDITY |
| | Infection risk | microbial load, temperature, immune index, CRP | weighted infection-risk score | INFECTION_RISK, MICROBIAL_LOAD |

**Recommended actions.** When a feature is raised, its card also shows an *Action* line taken from the resolution plan
(for example "Resistive + impact exercise, vitamin D and calcium management" for the bone, muscle, loading and nutrition features).

**Which features were added from the resolution plan.** Causes already highlighted in the plan (radiation dose, sleep, mood, bone/muscle
loss, fluid shift, CO2/O2, microbes, immune markers) were already covered. The un-highlighted causes were added as features:
nutrition, reduced mechanical loading, workload, confinement (social contact and Earth communication) and reluctance to report.

**How alerts behave.** A WARNING must repeat `--persistence` samples (CRITICAL is immediate). Once raised, an alert holds for
5 samples and only clears when the measure falls 10 % below its limit, which stops flicker around a threshold.
Fast signals use the astronaut's own first 30 samples as a baseline.

**Known limits.** All coefficients and thresholds are illustrative defaults for software testing, not clinical guidance, and
are plain numbers at the top of each evaluator in `ext_analysis.py`. Several signals (bone markers, cytokines, cognitive tests)
could not come from an implant in reality, so here they stand for periodic measurements. Circadian-phase drift is not modelled yet.
