# Nitch for Android TV

A thin Kotlin shell around a WebView (D4). The UI is the web app served by your Nitch server, so it
updates with the server; the APK only finds the server, forwards the remote's keys, and offers a
small bridge (`window.NitchTV`).

- **Server discovery:** mDNS (`_nitch._tcp`, announced by the server) or a typed address. The choice
  is remembered; on the emulator use `10.0.2.2:8080` (mDNS doesn't pass the emulator's NAT).
- **Remote:** D-pad and OK work through the web app's spatial navigation; Back and Play/Pause are
  passed to the page (`nitch:back`, `nitch:playpause`).
- **Bridge:** `exit()`, `setKeepScreenOn(on)`, `changeServer()`.
- No libraries besides the Android SDK; runs on Android TV (Leanback launcher + banner) and, for
  testing, on phones.

## Build

Open this folder (`android-tv/`) in **Android Studio** and press **Run**, or from a terminal:

```powershell
cd <repo>\android-tv
gradle wrapper --gradle-version 9.8.2   # once, creates gradlew (needs a Gradle install)
.\gradlew.bat assembleDebug             # → app\build\outputs\apk\debug\app-debug.apk
```

Versions: Gradle 9.8.2 (pinned in `gradle/wrapper/gradle-wrapper.properties` with its checksum),
Android Gradle plugin 9.5.0 with its built-in Kotlin support, compile/target SDK 37, min SDK 26,
JDK from Android Studio.

## Install on a TV

Enable developer options and ADB on the TV, then:

```powershell
adb connect <tv-ip>:5555
adb install -r app\build\outputs\apk\debug\app-debug.apk
```
