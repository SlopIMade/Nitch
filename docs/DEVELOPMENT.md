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
| GitHub CLI | when we connect GitHub | `winget install GitHub.cli` |
| VS Code + WSL + rust-analyzer extensions | optional | `winget install Microsoft.VisualStudioCode` |

For Android TV testing, open Android Studio's *Device Manager* and create a **TV** device (1080p,
x86_64 image).

## 4. Running on this PC

- **Development:** `cargo run` in WSL; the Vite dev server runs on Windows and proxies `/api` and
  `/hls` to the server. WSL2 forwards its ports to Windows `localhost`, so this works in both directions.
- **Container test:** `docker compose up` in WSL, the same image as production.
- **Android TV emulator:** reaches the server at `10.0.2.2:<port>`. mDNS doesn't pass through the
  emulator's NAT, so use manual entry there.
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
