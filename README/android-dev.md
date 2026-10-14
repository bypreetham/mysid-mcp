# Android development with mysid

How `mysid build`, `mysid release` and the device commands behave on an Android (Gradle) project, what they print
while a build runs, and where the live log goes. Most of this also applies to the other stacks `build` detects
(Cargo, npm, Python, a project `run.bat`).

Models learn these commands from `mysid skill android` (also installable as a Claude Code skill); the main `mysid help` lists
only the general flow and points here, so a React or Rust project never sees the Android commands.

## Commands

| Command | What it does |
|---|---|
| `mysid build` | Builds the project. Waits up to 20 s, then returns a progress line and keeps building. |
| `mysid build status` | Progress, or the final report. Waits up to 20 s for a change. |
| `mysid build stop` | Cancels the running build and everything it started. |
| `mysid build clean` | `clean` then `assembleDebug`. |
| `mysid build --fix-stale` / `--ignore-stale` | Before a Gradle build, mysid checks for caches made at another path (`app/.cxx/.../CMakeCache.txt` after the folder was moved) and for a `local.properties` `sdk.dir` that does not exist. Either stops the build with `NOT STARTED` and the cause, instead of failing minutes in; `--fix-stale` deletes the stale `.cxx` folders and goes on, `--ignore-stale` builds anyway. `mysid exec gradlew ...` is checked the same way. |
| `mysid build --wait N` | Wait up to N seconds before returning a progress line (also for `status`). |
| `mysid release` | Bumps `versionCode`, builds the release bundle (`bundleRelease`), verifies it. |
| `mysid debug "kw, kw"` | Clears the device log and records only matching log lines to `.mysid/debug.log` (see [Debugging](#debugging-with-device-logs)). |
| `mysid devices`, `mysid android <action>`, `mysid wifi` | ADB helpers: devices, install, launch, logcat, crashes, tap, type, keyevent. |
| `mysid install` | Android project: deploys the built debug APK to a connected device. Rust project: builds the release binary and installs it to `%LOCALAPPDATA%\mysid\bin` (and refreshes a copy in `~/.local/bin`); a running copy is renamed aside, so it works while the MCP server is up. |
| `mysid exec <command...>` | Runs a shell command in the workspace (30 s timeout by default, `--timeout N`). |

`build` and `release` both end with a short result; the complete Gradle output is always in `.mysid/build.log`.

## How a build runs

```
mysid build ──► writes .mysid/build.job.json ──► starts a detached worker ──► waits up to 20 s
                                                      │
                                       runs Gradle, streams every line to .mysid/build.log,
                                       rewrites .mysid/build.status.json every second
```

- **Gradle is run directly** (`gradlew assembleDebug --console=plain`). A project script such as `tools\run.bat` is
  only used when there is no `gradlew`, because those scripts usually redirect Gradle's output to a temp file and hide
  all progress. If the environment has no usable `JAVA_HOME`, the one pinned in `run.bat` (`set "JAVA_HOME=..."`) is used.
- **The worker is detached**, so the build outlives the command that started it (an LLM tool call has a time limit;
  a build may not). It does not inherit the caller's output pipes, so `mysid build 2>&1 | tail` returns at the wait
  budget instead of waiting for the build.
- **One build at a time per workspace.** Running `mysid build` again while one is running attaches to it.
- **Heartbeat.** The status file is refreshed every second; a `running` status that has not changed for 20 s is reported as
  a stopped worker.

## What you see

While the call is waiting, each stage change is printed to **stderr** (so MCP's stdout stays clean):

```
BUILD STARTED - Android (Gradle) - log: .mysid/build.log
[0:00] gradle startup: starting Gradle (daemon start and configuration print nothing until tasks begin)
[0:06] configuring
[0:08] :app:preBuild (resources/manifest)
[0:10] :app:kspDebugKotlin (annotation processing)
[0:12] :app:compileDebugKotlin (Kotlin compile)
[0:15] :app:buildCMakeDebug[arm64-v8a] (native C++)
[0:22] :app:packageDebug (packaging)
BUILD OK (Android/Gradle, 24s) - 10 tasks (9 executed), 5 warning(s)
  3x 'foo' is deprecated. Use bar instead  (app/src/A.kt:3:1, app/src/B.kt:9:5, +1)
  app/src/D.kt:2:2 Variable 'x' is never used
APK: app/build/outputs/apk/debug/app-debug.apk (45.2 MB)
stages: gradle startup 6s | configuring 2s | Kotlin compile 3s | native C++ 3s | ...
log: .mysid/build.log
```

If the build is still going when the wait ends:

```
BUILD RUNNING 2m10s - at :app:kspDebugKotlin (annotation processing, 40s in stage) - 62% (~410/660 tasks) - 1 warning(s)
log: .mysid/build.log
check: mysid build status   cancel: mysid build stop
```

- The percentage comes from the task count of the last successful build of the same stack (`.mysid/build.stats.json`).
- `N error(s) so far` appears as soon as Gradle prints an error, so an agent can stop early.
- **Failure** shows the failed task, the errors (grouped by message) and Gradle's "What went wrong" cause. Warnings are hidden.
  The CLI exits with status 1.
- **Warnings** are grouped by message across files: `3x 'foo' is deprecated (A.kt:3:1, B.kt:9:5, +1)`, top 6 kinds, then
  `+N more kinds (full list: .mysid/build.log)`.

### Stages

Gradle task names are mapped to stages; tasks that match none keep the current stage.

| Stage | Task names containing |
|---|---|
| native C++ | `externalNativeBuild`, `cmake`, `ndk` |
| annotation processing | `kapt`, `ksp` |
| Kotlin compile | `compile` + `kotlin` |
| Java compile | `compile` + `java` |
| R8 shrink | `r8`, `minify`, `shrink`, `proguard` |
| dexing | `dex` |
| lint / tests | `lint` / `test` |
| packaging | `nativeLibs`, `strip`, `package`, `assemble`, `bundle`, `sign`, `zipalign` |
| resources/manifest | `manifest`, `resource`, `aapt`, `databinding`, `buildConfig`, `metadata`, `preBuild`, `generate`, `merge`, `check` |

mysid also names the steps it performs itself: `preflight`, `version bump`, `gradle startup`, `configuring`, `verifying`.
The `stages:` line at the end gives the time spent in each (stages under 2 s are omitted). On one real release,
R8 shrink took 2m21s of 3m58s, which is where tuning would pay off.

## mysid release

`mysid release` takes no flags. Every run:

| Step | What happens |
|---|---|
| **preflight** | Reports the JDK (`JAVA_HOME`) and the environment variables your Gradle files read (`System.getenv("...")`, usually signing): `signing env: 0/4 set (not set: ...; fine if build.gradle falls back to another keystore)`. Reported, not enforced, because many projects fall back to another keystore. A set keystore path that does not exist is flagged. |
| **version bump** | Raises `versionCode` by 1. |
| **gradle startup / build stages** | `gradlew bundleRelease -x test`, with the stages above. |
| **verifying** | Checks the bundle is a valid zip, runs `jarsigner -verify`, and checks the R8 mapping file. |

```
RELEASE OK (Android, 3m58s) - versionCode 39 -> 40, 54 tasks (19 executed), 2 warning(s)
AAB: app/build/outputs/bundle/release/app-release.aab (72.0 MB)
R8 mapping: present (81.8 MB)
stages: configuring 8s | resources/manifest 12s | packaging 39s | R8 shrink 2m21s
Signature: verified
```

### The version bump

- It edits only the number, in `build.gradle.kts` (`versionCode = 37`), `build.gradle` (`versionCode 37`) or an old-style
  manifest (`android:versionCode="37"`), and sets all of them to the highest current value + 1. Everything else in the file
  stays byte-identical, including CRLF line endings.
- It happens **inside the worker, right before Gradle starts**. A release already running is attached to, never bumped twice.
- **A failed or cancelled build puts the number back** (`versionCode restored to 38`); a successful one keeps it.
- A computed value (`versionCode = base + 1`) is left alone and reported: `versionCode not bumped: no literal versionCode found`.
- `mysid build` never bumps.

### Verification results

| Line | Meaning |
|---|---|
| `Signature: verified` | `jarsigner` accepted the bundle. |
| `Signature: UNSIGNED - Google Play will reject this bundle` | build finished but nothing signed it. |
| `Signature: not checked (...)` | no bundle, not a valid zip, or no `jarsigner` (set `JAVA_HOME`). |
| `R8 mapping: EMPTY (0 bytes) ...` | R8 wrote no mapping, so release crash reports cannot be de-obfuscated. |
| `R8 mapping: missing` | no `mapping.txt` under `*/build/outputs/mapping/release/`. |

## Debugging with device logs

`mysid debug` records the lines your own `Log.d` / `LOGD` calls print, filtered by keywords, so an agent reads a few
relevant lines instead of the whole logcat.

```
mysid debug "Buffer, router, usb, BT, decoder"
```

1. Runs `adb logcat -c` (clears the device log) and empties the previous recording.
2. Starts a detached recorder (`mysid __debug_recorder`) that streams `adb logcat -v threadtime`.
3. Saves only the lines whose **tag or text** contains any keyword (case-insensitive) to `.mysid/debug.log`, as they
   arrive. Keywords are comma-separated (so phrases work: `"audio track, usb"`), or space-separated without commas.
4. Records for a **fixed window, 20 seconds by default** (`--seconds N` or `-N`, clamped to 10-60), while you reproduce
   the problem. The call waits for it, the recorder then stops by itself, and the result comes back in one reply
   that starts with `20 sec log record done`.

```
20 sec log record done - 133 line(s) saved to .mysid/debug.log
hits: Buffer 41, router 3, usb 0 | 2 error, 5 warning
12:34:56.789 D UsbBulkEngine: [USB] opened fd 7
12:34:56.801 D AudioBuffer: fill 480  (x12)
12:34:57.002 E UsbBulkEngine: Buffer underrun at 44100
```

A keyword call always starts a **new** window (and clears the previous recording). To look at the last recording again:

| Command | Result |
|---|---|
| `mysid debug` | last 60 recorded lines |
| `mysid debug --new` | only lines since the previous `mysid debug` call |
| `mysid debug --level W` | warnings and errors only (`V D I W E`) |
| `mysid debug --lines 100` | up to 100 lines |
| `mysid debug status` / `stop` / `clear` | recorder state / stop it early (log kept) / empty the log |

### Raw mode: system errors, not just your logs

The keyword recorder sees whatever the device logs, but a problem is not always printed by your own `Log.d` / `LOGD`
(a framework error, an `AudioFlinger` warning, a native abort). `--raw` captures the **whole** device log for a short
window and filters it afterwards:

```
mysid debug error --raw             # 20 seconds by default
mysid debug error --raw 40          # any value is clamped to 10-60
mysid debug "underrun, xrun" --raw 30
mysid debug error -raw -20          # single-dash spelling works too
```

1. Clears the device log (`adb logcat -c`) and stops a running keyword recorder (they share `debug.log`).
2. Streams `adb logcat -b all -v threadtime` (every buffer: main, system, crash, events, radio; falls back to the default
   buffers if the device refuses `-b all`) for the window while you reproduce the problem.
3. Keeps every line **containing a term anywhere** (tag or text, including system tags) with **3 lines above and 3 below**;
   overlapping windows merge into one block and blocks are separated by `# --`.
4. Saves the result to `.mysid/debug.log` (replacing it) and prints `N sec log record done` with the last 60 lines,
   hit lines marked with `*`.

```
20 sec log record done (raw): 13492 device lines captured, 76 matching line(s) in 27 block(s); saved to .mysid/debug.log (43 KB)
hits: error 76
  10-08 21:12:47.292 I am_proc_bound: [0,13546,com.oplus.securitypermission]
* 10-08 21:12:47.655 E OemSimProtect: checkPrivacyProtect error.
  10-08 21:12:47.655 D OemSimProtect: checkPrivacyProtect :contact/...
```

A term is required. `mysid debug` with no arguments shows the saved result again. Use raw mode to find out *what* is wrong,
then switch to `mysid debug "tag, tag"` to follow it in your own tags.

- The file holds one recording at a time: a new keyword call replaces it.
- A keyword with `0` hits is usually a typo or code that was not reached.
- The recorder reconnects if the device drops during the window and rolls the file over at 32 MB (`debug.log.old`). Set `ANDROID_SERIAL` to choose a device. Messages are clipped at 220 characters
  here; the file has them whole.
- Follow it live: `Get-Content .mysid\debug.log -Wait`.

## Files in `.mysid/`

| File | Content |
|---|---|
| `debug.log` | Recorded, keyword-filtered logcat lines (`mysid debug`). Live; emptied when a new recording starts. |
| `debug.job.json`, `debug.state.json`, `debug.touch`, `debug.cursor` | recorder keywords, heartbeat/status, last-use time (idle shutdown), read position for `--new`. |
| `build.log` | **Live, complete output** of the current/last build: a header with the command, every line as it arrives, and `# exit code: N` at the end. Overwritten by each build. Follow it with `Get-Content .mysid\build.log -Wait` (PowerShell) or `tail -f .mysid/build.log`. |
| `build.status.json` | current state: `running` / `ok` / `failed`, current task and stage, counters, the stage trail, versionCode bump info, and the final report text. |
| `build.job.json` | what the worker was asked to run. |
| `build.stats.json` | task count and duration of the last successful build per stack (used for the percentage). |

See [agent-data.md](agent-data.md) for the full layout of mysid's data.

## Other stacks

`mysid build` detects the stack: `gradlew` (Android), `package.json` (`tsc --noEmit`, then `npm run build`), `Cargo.toml`
(`cargo build`, with `mysid build --release`), `pyproject.toml`/`setup.py`, or a project `run.bat`. They share the worker,
the log and the status. Cargo builds print `BUILD OK (Rust, 6.7s) - 7 warning(s)` with one `file:line warning: ...` per
warning (or the errors only on failure).

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| Nothing printed for a long time at `gradle startup` | Gradle prints nothing during daemon start and configuration. The stage counter shows time in stage. |
| `BUILD FAILED ... worker stopped responding` | the worker died; see `.mysid/build.log` for the last output. |
| `mysid build stop` then the version is back | by design: a cancelled release restores `versionCode`. |
| `Signature: UNSIGNED` | check `signing env:` in the preflight line, and your `signingConfigs`. |
| A different project builds | mysid takes the project from the folder you run it in; check with `mysid get_workspace`, then `cd` into the project, pass `--workspace_root <path>`, or run `mysid set <path>` there. |

## Drive the UI: tap, swipe, type (every action returns a screenshot)

```
mysid android screenshot                       # just the screen
mysid android tap --text "Widgets"             # find the element and tap its center (--desc "Open menu", --id <resource-id>)
mysid android tap --x 540 --y 1200             # or device pixels (also: tap 540 1200)
mysid android swipe --x1 540 --y1 1800 --x2 540 --y2 600
mysid android type --text "hello world"        # newline = Enter
mysid android keyevent back                    # back, home, enter, recent, KEYCODE_..., or a number
mysid android ui_dump                          # [i] text | content-desc | id | clickable | center(x,y)
mysid android keyevent dpad_right --times 20 --fast   # 20 presses in ONE adb call (~1.7 s per call on a TV); --keys "dpad_right dpad_right enter"; --gap 50 = ms between presses
mysid android focus                            # text only: the focused element - label, class, center, bounds, path (TV / D-pad)
mysid android focus --list                     # ... plus every focusable element on screen (* = focused)
mysid android goto "USB Drive" "Durandhar"     # D-pad until each label has focus, then OK; --no-ok only moves focus, --max 30 presses per target
mysid android keyevent dpad_right --no-shot    # one key press answered with text: result + focus line, no screenshot
```

One call per step: the action runs, mysid waits `--wait` ms (default 600) for animations, captures the screen and answers with one line:

```
tap(540,1200) ok | device dee50561 | screen 1080x2412 | image 896x2000 | scale 1.205
```

- **The image is smaller than the screen.** Coordinates are **device pixels**; device = image x scale (here 1.205). To use numbers read off the image, add `--space image` and mysid converts.
- `--grid` draws a labelled 100 px grid (device pixels) and a red crosshair where the action landed: use it to check a tap. `--scale 0.4` makes a smaller image (default: longest side 2000 px).
- `tap --text/--desc/--id` finds the element with uiautomator (Compose icons usually have only a content description). Several matches: nothing is tapped and the candidates are listed; pick with `--index n`.
- Several devices connected: `--device <serial>`. An out-of-range coordinate is refused, not tapped. A protected (FLAG_SECURE) screen is reported instead of returning a black image.
- Several steps in one call: `mysid android batch --json steps.json` with `[{"action":"tap","text":"Widgets","shot":true},{"action":"wait","ms":500},{"action":"keyevent","keycode":"back"}]`. Screenshots come back for steps with `"shot": true` and for the last step.
- On the command line the PNG is `.mysid/shots/<ms>.png` (last 20 kept); over MCP it is an image block. Every action is logged in `.mysid/touch.log`; every adb call times out after 10 s.

### Safer driving: checks that catch a wrong tap

- Every result names the **foreground app**, the **workspace**, the full path of the saved PNG, and whether the **screen changed** or is **UNCHANGED** after the action (`--no-diff` skips the extra capture).
- `--app com.example.app` refuses to act when another app is in front: nothing is tapped, and the answer says which app is there. Use it on every step of a run.
- `--expect "Settings"` waits (up to `--timeout` ms, default 5000) until that text or description is on screen, instead of a fixed `--wait`, so a slow animation does not leave a half-drawn screenshot.
- `scroll down|up|left|right` swipes for you; `scroll down to "Battery"` keeps scrolling until the text is on screen (up to `--max` 8 times).
- A black screenshot is explained: `the screen is off` (wake it with `keyevent wakeup`), `the phone is locked` (unlock it by hand), or a protected window.
- In `batch`, a failed step, a failed `expect`, or (with `"require_change": true`) an unchanged screen **stops the run**, so later taps never hit whatever is there.
### Scripted scenarios with checks: `mysid android scenario steps.json`

One file, one run, one report. Rerun it after every change and each fix becomes a regression check.

```json
{
  "name": "usb-playlist",
  "app": "com.example.tv",
  "max_cold_start_ms": 3000,
  "max_janky_percent": 10,
  "steps": [
    "launch --cold",
    "expect text 'USB Drive' --timeout 15000",
    "goto 'USB Drive' 'Durandhar'",
    "expect text 'Play all'",
    "expect focus 'Play all'",
    "key dpad_center",
    "expect gone 'Loading'"
  ]
}
```

Steps are command lines (quotes group words) or objects (`{"action":"expect","focus":"Play all","timeout":8000}`):

| step | does |
|---|---|
| `launch [--cold] [package]` | `--cold` force-stops first; starts with `am start -W` and records `TotalTime` |
| `key <k> [--times n]`, `tap`, `swipe`, `type`, `scroll`, `wait <ms>` | the same actions as on the command line (a key press waits for the screen to settle) |
| `goto <labels...> [--no-ok]` | D-pad to each label, then OK (see above) |
| `expect text <t>` / `focus <t>` / `gone <t>` / `app <pkg>` | polls (default 5 s, `--timeout ms`) until the text is on screen, the element has focus, the text is gone, or the app is in front |

The report:

```
scenario usb-playlist FAIL - step 4 of 7: expect text 'Play all'
 1 PASS launch --cold              cold start 812 ms (com.example.tv/.MainActivity)
 2 PASS expect text 'USB Drive'    text "USB Drive" found after 1.2s
 3 PASS goto 'USB Drive' 'Durandhar'  goto "Durandhar": focused after 3 press(es) ...
 4 FAIL expect text 'Play all'     text "Play all" is not on screen (visible: Home | Albums ...) after 5.0s
      ! CRASH com.example.tv: java.lang.IllegalStateException: no folder (at com.example.tv.Usb.open(Usb.kt:42))
      saved: .mysid/shots/1791880785660.png
 5 SKIP ...
cold start: 812 ms (am start -W TotalTime)
frames: 340 rendered, 12 janky (3.5%), p90 14ms, p95 22ms, p99 40ms
crashes / ANRs / exceptions: 1 (listed under the step that was running)
```

- Only the **first failing step** gets a screenshot. Later steps are skipped unless `--keep-going` (or `"keep_going": true`).
- **Crashes, ANRs and exceptions** come from logcat (errors and worse), cleared after every step, so each event belongs to the step that was running. A Java crash (`FATAL EXCEPTION`), an ANR or a native crash of the app's own process fails its step (`"allow_crash": true` on a step to expect it); a logged exception from the app's process is listed but does not fail. Other apps' crashes are ignored.
- **Cold start** is `TotalTime` of the first `launch --cold`. **Frames** are `dumpsys gfxinfo` since the scenario began (the counters are reset first; a cold launch starts them again).
- `"max_cold_start_ms"` and `"max_janky_percent"` in the file turn those numbers into pass/fail (`BUDGET:` lines). The exit status is non-zero on FAIL, so it can run in CI.
- The package comes from `"app"` in the file, else `--app`, else the app in front when the run starts. Each step is also logged in `.mysid/touch.log`.

- **No touch (TV).** `tap --text` does nothing on a TV; drive it with the D-pad. `focus` reads the accessibility tree and says which element has focus. `goto "A" "B"` reads the tree before every press, picks the key towards the target (same row: sideways, otherwise up/down; the other axis after a press that did nothing), presses OK on arrival, and stops with a reason when focus does not move, jumps the wrong way, goes in circles, or the label is not on this screen (a screenshot is attached then). It answers with text and the final focus line, so no screenshot is needed. A key press waits until the screen stops changing (two equal captures in a row, `--settle-timeout`, default 2000 ms) instead of a fixed 600 ms; `--wait` brings the fixed wait back. `focus` and `goto` also work as batch steps.
- **Fast keys.** `--fast` sends every press in one `input keyevent K K K ...` call (up to 200), so seek-by-keys tests take seconds instead of a minute; `--gap ms` runs an on-device loop with that pause when the app drops presses sent back to back. Several keys in one call need Android 7 or newer.
- **Rows labelled by an index.** `goto` matches text anywhere inside a focusable row (a title inside a cell labelled "2"). When several rows match, the answer says `(1 of 6 matches; --nth n picks another)`; `goto "Song" --nth 3` takes the third in screen order, top to bottom.
- **The right app.** `--app <package>` refuses to send keys, taps or `goto` unless that app is in front (nothing is sent). A scenario applies its `"app"` to every step by default; a step can name another app or use `--any-app`.
- `ui_dump` calls are serialised and retried once when uiautomator says "already registered"; `keyevent back --times 2` presses twice (max 10).
