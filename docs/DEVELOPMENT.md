# Development setup

| | Machine |
|---|---|
| **Development** | A Windows PC with WSL2 |
| **Production** | Linux VM with an AMD iGPU passed through via QEMU VirGL |

The Linux parts (Rust server, GStreamer, Docker) run in **WSL2**, the same environment the production
container uses. The web client and the Android app are built on Windows.

## 1. Enable virtualization (once)

If `systeminfo` reports virtualization in firmware as disabled, turn it on: WSL2, Docker and the
Android emulator all need it.

1. Reboot into the UEFI/BIOS and enable **SVM Mode** (AMD-V). It is usually under
   *Advanced → CPU Configuration*, or *OC Tweaker* on ASRock boards.
2. Back in Windows, in an **admin** PowerShell:

   ```powershell
   wsl --install -d Ubuntu-24.04
   ```

   `wsl --list --online` shows whether a newer Ubuntu LTS is available. Whichever release we pick
   becomes the base of the production image too, so the GStreamer versions match.

3. Only needed for the Android emulator (M5), also in an admin PowerShell:

   ```powershell
   dism /online /enable-feature /featurename:HypervisorPlatform /all /norestart
   ```

4. Reboot.

The WSL distro lives on the system drive. The source code can stay on any Windows drive, while
build output goes into WSL (see below).

## 2. Inside WSL (Ubuntu)

Open an Ubuntu terminal and run the setup script ([scripts/setup-wsl.sh](../scripts/setup-wsl.sh)).
It asks for your sudo password and is safe to run again. It does the following:

- installs the build tools, the GStreamer dev libraries and plugins, VA-API tools, Docker Engine and Compose
- adds you to the `docker` group
- installs Rust via rustup
- sets `CARGO_TARGET_DIR` in `~/.profile`, so build output stays on the fast Linux disk instead of `/mnt/<drive>`

```bash
bash <repo>/scripts/setup-wsl.sh
```

Then run `wsl --shutdown` in Windows and reopen WSL, so the docker group and `PATH` take effect.
Verify with the check script, which changes nothing:

```bash
bash <repo>/scripts/check-dev.sh
```

## 3. Windows side

| Tool | Needed from | Install |
|---|---|---|
| Node.js LTS | M1 (spike page), M2 (web app) | `winget install OpenJS.NodeJS.LTS` |
| Android Studio (SDK, emulator, JDK) | M5 | `winget install Google.AndroidStudio` |
| GitHub CLI (installed, logged in) | pushing to `SlopIMade/Nitch` | `winget install GitHub.cli` |
| VS Code + WSL + rust-analyzer extensions | optional | `winget install Microsoft.VisualStudioCode` |

For Android TV testing, open Android Studio's *Device Manager* and create a **TV** device (1080p,
x86_64 image).

## 4. Running on this PC

Credentials first: copy `.env.example` to `.env` and fill in the Twitch app's Client ID and secret.

- **Server** (WSL, from the repository root, so `.env` and `web/dist` are found):

  ```bash
  cd <repo> && cargo run --manifest-path server/Cargo.toml -p nitch-server
  ```

  It listens on `http://localhost:8080` and serves the built web app from `web/dist`. The login and
  other data go to `data/nitch.db`.
- **Web app** (Windows, in `web/`):
  - `npm run build` builds the app that the server serves.
  - `npm run dev` starts a Vite dev server on `http://localhost:5173` with hot reload; it proxies
    `/api` and `/hls` to the server.

  WSL2 forwards its ports to Windows `localhost`, so both directions work.
- **Container** (WSL, the same image as production):

  ```bash
  cd <repo> && docker compose -f deploy/compose.yaml up -d --build
  ```

  The container keeps its own login in the `nitch-data` volume. `NITCH_BIND=0.0.0.0:8090` in front
  of the command runs it next to a dev server that's already on 8080.
- **Android TV emulator:** reaches the server at `10.0.2.2:8080`, and also at the WSL address that
  mDNS announces (e.g. `172.24.x.x`). After updating the web app, the TV app clears its cache on start,
  and the server sends `index.html` with `no-cache`. An old cached copy pointed at deleted
  scripts and gave a black screen.
- **A real TV against the server in WSL:** the WSL address (`172.24.x.x`) only exists on this PC.
  A TV on the LAN needs WSL's mirrored networking, so the server is reachable at the PC's
  LAN address. Put this in `%UserProfile%\.wslconfig`, then run `wsl --shutdown`:

  ```ini
  [wsl2]
  networkingMode=mirrored
  ```

  You may also need to allow port 8080 in the Windows firewall. On the target VM none of this applies.
- **Clock under WSL:** the monotonic clock in WSL2 can run several percent slow. Multiview
  pipelines therefore run on a clock calibrated to the wall clock's rate ([D26](DECISIONS.md#d26--multiview-pipelines-keep-real-time-on-a-calibrated-clock)).
- **Encoding on this PC:** software x264 only. WSL exposes the GPU as `/dev/dxg` but has no
  `/dev/dri`, and GStreamer's VA plugin needs `/dev/dri`. That's fine: the 3900X has plenty of
  headroom, and the GPU path that matters has to be verified on the target VM anyway.

## 5. Checks on the target server (Linux VM)

Run inside the VM and send the output:

```bash
sudo apt install -y vainfo   # Debian/Ubuntu, if missing
vainfo
lscpu | head -20
free -h
```

`vainfo` lists the H.264 entry points VirGL exposes. `VAEntrypointVLD` means hardware decode;
`VAEntrypointEncSlice` or `VAEntrypointEncSliceLP` means hardware encode. Also tell me:

- the hypervisor (Proxmox, libvirt/virt-manager, plain QEMU, …)
- the host's APU model
- how many cores and how much RAM the VM gets

## Releases

A version tag publishes a release (`.github/workflows/release.yml`):

```bash
git tag v0.2.0
git push origin v0.2.0
```

- **Docker Hub:** the image as `slopimade/nitch:0.2.0`, `:0.2` and `:latest` (x86-64). Tags with a
  suffix (`v0.3.0-rc1`) are pre-releases and don't move `:latest`.
- **GitHub release:** `nitch-tv-0.2.0.apk`, signed with the release key; its versionCode comes
  from the version (0.2.0 → 200), so Android installs it over the previous one.
- **Test run:** Actions → Release → Run workflow builds both without publishing anything.

Secrets, set once (Settings → Secrets and variables → Actions):

- `NITCH_KEYSTORE_BASE64`, `NITCH_KEYSTORE_PASSWORD`, `NITCH_KEY_PASSWORD`: run
  `.\scripts\create-apk-key.ps1 -Out <a safe folder>`. It creates the key, stores it as secrets and
  leaves the key and its password in that folder. Back them up: without the key, the next APK
  can't update the installed one.
- `DOCKERHUB_TOKEN`: a Docker Hub access token (Account settings → Personal access tokens,
  Read & Write) for the `slopimade` account: `gh secret set DOCKERHUB_TOKEN --repo SlopIMade/Nitch`
  asks for it.

