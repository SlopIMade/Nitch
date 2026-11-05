# Installing Nitch

Nitch runs as one Docker container on a Linux machine in your home network. Desktop browsers open
it directly; the Android TV app finds it on the network.

## 1. Register a Twitch app (once)

Login, followed channels and search use Twitch's official API, which needs an app of your own:

1. Open [dev.twitch.tv/console](https://dev.twitch.tv/console). Twitch requires two-factor
   authentication on your account for this.
2. **Register Your Application:**
   - **Name:** anything, e.g. `Nitch personal`. You see it when you confirm the login on your phone.
   - **OAuth Redirect URL:** `https://localhost`. It isn't used, but the field is required.
   - **Category:** Application Integration
   - **Client Type:** **Confidential**, so the login doesn't expire.
3. Open **Manage**, copy the Client ID, and create a **New Secret**.

## 2. Configure

Copy `.env.example` to `.env` next to the repository and fill in:

```
NITCH_TWITCH_CLIENT_ID=...
NITCH_TWITCH_CLIENT_SECRET=...
```

Optional settings (environment variables):

| Variable | Default | Meaning |
|---|---|---|
| `NITCH_BIND` | `0.0.0.0:8080` | Address and port |
| `NITCH_ENCODER` | `x264` | `va` for VA-API hardware encoding (falls back to x264 if missing) |
| `NITCH_X264_PRESET` | `veryfast` | x264 speed preset |
| `NITCH_MAX_MULTIVIEW` | `2` | Multiviews running at once (~2 cores each in software) |
| `NITCH_MDNS` | on | `0` turns off the network announcement for the TV app |
| `RUST_LOG` | `info` | Log level |

## 3. Start

The image is on Docker Hub (`slopimade/nitch`, x86-64). Put `deploy/compose.yaml` and your `.env`
in one folder (or use the repository as it is) and start it:

```bash
docker compose -f deploy/compose.yaml up -d
```

To build the image from source instead:

```bash
docker compose -f deploy/compose.yaml -f deploy/compose.build.yaml up -d --build
```

- **Host networking:** the compose file uses it, so the server is reachable on the LAN without port
  mapping, and the mDNS announcement reaches the TV.
- **Data:** login, sessions and multiview output live in the `nitch-data` volume.
- **Hardware encoding:** uncomment `devices: [/dev/dri:/dev/dri]` in `deploy/compose.yaml` and set
  `NITCH_ENCODER=va`. Check first that `vainfo` inside the machine lists H.264 `VAEntrypointEncSlice`.

Then open `http://<server>:8080` in a browser and click **Log in**. Scan the QR code with your
phone, press **Authorize**, and wait for "Success".

**Security:** Nitch is meant for your home network only. Don't forward its port to the internet.
The server holds the Twitch login, and any device on the network can use it.

## 4. The TV app

1. Download `nitch-tv-<version>.apk` from the
   [latest release](https://github.com/SlopIMade/Nitch/releases/latest), or build it yourself (see
   `android-tv/README.md`).
2. Install it on the TV: enable developer options and network ADB, then:

   ```powershell
   adb connect <tv-ip>:5555
   adb install -r nitch-tv-<version>.apk
   ```

   Or copy it to the TV (USB stick, a file-sending app) and open it with a file manager.

3. Open **Nitch** from the TV's app row. It lists the servers it finds on the network. Otherwise
   enter the address (e.g. `192.168.1.10:8080`).

## Example: a home server with an NFS share and VA-API

`deploy/stack.yaml` is a template for this (D32): host networking, VA-API through `/dev/dri`,
the data on an NFS share, the multiview output in memory. Everything machine-specific comes
from `.env` (or Portainer's stack variables): `NITCH_NFS_ADDR` and `NITCH_NFS_PATH` (the folder
must exist), `NITCH_RENDER_GID` and `NITCH_VIDEO_GID` (`getent group render video`).

1. Copy the repository to the server (e.g. `~/nitch`). If its GPU needs a Mesa build of its own,
   e.g. a VM's virtio GPU in AMD native-context mode, put those packages into `deploy/debs/`:
   `mesa-va-drivers`, `mesa-libgallium`, `libegl-mesa0`, `libgbm1`, `libglx-mesa0` and
   `libgl1-mesa-dri` (all of them: they require each other's exact version).
2. Build the image and start the stack:

   ```bash
   sudo docker build -t nitch:latest -f deploy/Dockerfile .
   sudo docker compose -p nitch --env-file .env -f deploy/stack.yaml up -d
   ```

   In Portainer, the stack can be created from the same file instead (Twitch credentials as stack
   environment variables); the image has to be built on the server first.
3. Check: `sudo docker exec nitch-nitch-1 vainfo --display drm --device /dev/dri/renderD128` lists
   `VAEntrypointEncSlice` for H.264, and the log says `encoder=Va`.

## Updating

```bash
docker compose -f deploy/compose.yaml pull
docker compose -f deploy/compose.yaml up -d
```

Built from source: `git pull`, then the build command from step 3 again.

The TV app doesn't need updating for UI changes: it loads the web app from the server.
