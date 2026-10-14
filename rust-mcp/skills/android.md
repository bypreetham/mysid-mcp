---
name: mysid-android
description: Build, release, install and debug an Android (Gradle, Kotlin/Java/C++) app with mysid: staged builds, versionCode-bumping release, keyword and raw device-log capture. Use for any Android task in a workspace with mysid.
---
# mysid: Android workflow

Read `mysid-base` first (file names, search, read, patch). This adds building, releasing and device debugging.

## Build

```
mysid build            # waits ~20 s, then a progress line; the build keeps running
mysid build status     # waits for a change; shows the stage, e.g. at :app:kspDebugKotlin (annotation processing)
mysid build stop       # cancel
mysid build clean      # clean + assembleDebug
```

- Success prints `BUILD OK`, the APK path and the slowest stages. **Failure prints only the errors** and Gradle's cause (warnings are hidden). Fix the errors, run `mysid build` again.
- Never run `./gradlew` yourself: mysid runs it detached, streams everything to `.mysid/build.log`, and keeps the call short.
- A long first build is normal. Keep calling `mysid build status` instead of starting another build.

## Release

```
mysid release                 # raises versionCode by 1, builds the release bundle (.aab), verifies it
```

Bumps `versionCode` by 1 (restored if the build fails or is stopped), builds `bundleRelease`, then verifies the bundle: signature, R8 mapping file. Read the last lines: `Signature: verified`, `R8 mapping: present`. `UNSIGNED` means the signing setup is missing; tell the user.

## Devices: check, connect over Wi-Fi

```
mysid devices                 # connected devices (USB or Wi-Fi) and their state: device / unauthorized / offline
mysid wifi                    # switch a USB-connected phone to Wi-Fi, then the cable can go
mysid wifi --ip 192.168.1.20  # same, when the phone's IP is not detected (add --port 5555 for another port)
```

- `mysid devices` first. `unauthorized` means the phone is waiting for you to accept the USB debugging prompt; `offline` means unplug and replug.
- `mysid wifi` needs the phone on USB and on the same Wi-Fi as this computer. It sets adb to TCP port 5555, reads the phone's Wi-Fi IP,
  and connects to `<ip>:5555`. With several devices set `ANDROID_SERIAL` to choose one.

## Install and run on a device

```
mysid install                 # deploy the built debug APK
mysid android launch_app
mysid android crashes         # recent fatal exceptions
mysid android logcat --lines 100
```

## Debug with the device log

The programmer's `Log.d` / `LOGD` lines are the evidence. Record only the ones you need:

```
mysid debug "Buffer, router, usb, BT, decoder"      # clears logcat, records 20 s, then: "20 sec log record done"
```

Ask the user to reproduce the problem while it records. The reply has hit counts per keyword (a `0` means a typo or code that never ran) and the lines; the file is `.mysid/debug.log`.

| Need | Command |
|---|---|
| Longer or shorter window (10-60 s) | `mysid debug usb --seconds 40` |
| Read the last recording again | `mysid debug` (`--level W` for warnings and errors) |
| System or framework errors not logged by the app | `mysid debug error --raw 20`: whole device log, keeps matches plus 3 lines above and below |
| Native crash / ANR | `mysid debug "fatal, signal, ANR" --raw 30` |

Typical loop: `debug` to find what is wrong, `graph`/`read` to find the code, `patch`, `build`, `install`, `debug` again to confirm.

## Examples

```
mysid search "mIsIso" --types cpp,h
mysid graph UsbNativeStreamer --depth 2
mysid read UsbNativeStreamer.cpp#L120-180
mysid patch UsbNativeStreamer.cpp "bufferFrames = 128" "bufferFrames = 256"
mysid build
mysid install
mysid debug "underrun, buffer, usb"
mysid commit "Increase USB buffer to 256 frames"
```

## Things to know

- A C++ function called only from Kotlin/Java through JNI shows no callers in `graph` (names are linked within a language family). Use `mysid search "nativeName"` to find the Java side.
- Vendored folders (`external/`, `third_party/`) are skipped by search and graph; to read a file there, give its full path.
- `mysid release` changes `versionCode` in the Gradle file; tell the user, and commit it with `mysid commit` if they want it saved.

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
