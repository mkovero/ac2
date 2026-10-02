# Installing ac2

ac2 ships three programs:

| program | what it is |
|---|---|
| `ac2-ui` | the desktop app (shown as **ac2** in menus). It can host its own daemon. |
| `ac2d` | the daemon: owns the audio interface, runs the measurements. |
| `ac2` | the command-line client: set up sessions and measurements, script everything. |

Release artifacts are on the [GitHub releases page](https://github.com/mkovero/ac2/releases),
with a `SHA256SUMS` file. Check a download with `sha256sum -c SHA256SUMS --ignore-missing`
(Linux), `shasum -a 256 -c SHA256SUMS --ignore-missing` (macOS) or
`Get-FileHash <file>` (Windows PowerShell).

The goal is a clean machine to a first live transfer function in under two minutes. Each OS
section ends at a running daemon; [First measurement](#first-measurement) is the same on all
three.

## Linux (x86-64, glibc 2.35 or newer)

**Tarball** (all three programs, desktop entry, icon, systemd user unit):

```sh
tar xzf ac2-<version>-linux-x86_64.tar.gz
cd ac2-<version>-linux-x86_64
./install.sh                                      # into ~/.local; --prefix /usr/local with sudo
systemctl --user daemon-reload
systemctl --user enable --now ac2d                # the daemon, now and at every login
```

`install.sh --uninstall` (same `--prefix`) removes it again. Make sure `~/.local/bin` is on
your `PATH`; most distributions add it once it exists (log out and in).

The daemon uses the platform audio host (ALSA; PipeWire and PulseAudio through their ALSA
plugins) unless a session names JACK. JACK, including PipeWire-JACK, is loaded at run time:
`ac2 session open --backend jack …` works wherever `libjack.so.0` is installed.

**AppImage** (the desktop app only, with its embedded daemon):

```sh
chmod +x ac2-ui-<version>-x86_64.AppImage
./ac2-ui-<version>-x86_64.AppImage
```

The AppImage bundles no system libraries; it needs glibc, libstdc++ and libasound, which
every desktop has. Without FUSE, run it with `--appimage-extract-and-run`.

Real-time scheduling: the audio thread asks for real-time priority. With PipeWire this is
granted through rtkit; with plain ALSA or JACK add yourself to the `audio` (or `realtime`)
group your distribution configures in `/etc/security/limits.d`.

## macOS (11 Big Sur or newer, Apple silicon and Intel)

1. Open `ac2-<version>-macos-universal.dmg` and drag **ac2** to Applications.
2. Optional, for the command-line tools: copy `bin/ac2` and `bin/ac2d` from the disk image
   to a directory on your `PATH`:
   ```sh
   sudo cp /Volumes/ac2*/bin/ac2 /Volumes/ac2*/bin/ac2d /usr/local/bin/
   ```
3. Start **ac2** from Applications. macOS asks for microphone access the first time an input
   is opened; allow it (ac2 reads your audio interface's inputs, nothing else).

Unsigned builds (dry runs, or a release before the project has a Developer ID): macOS refuses
to open them at first. Open **System Settings → Privacy & Security** and click **Open
Anyway** for ac2, or run `xattr -dr com.apple.quarantine /Applications/ac2.app`. For the CLI
tools: `xattr -d com.apple.quarantine /usr/local/bin/ac2 /usr/local/bin/ac2d`.

The daemon can run as a launchd agent: `launchd/io.github.mkovero.ac2d.plist` in the disk
image has the instructions in its header. A daemon started by launchd cannot show the
microphone prompt, so either let the app host its daemon or start `ac2d` from Terminal.

## Windows (10 1809 or newer, x64)

1. Run `ac2-<version>-windows-x64.msi`. It installs into `C:\Program Files\ac2`, adds a
   Start-menu entry **ac2**, and puts the install folder on the system `PATH` (open a new
   terminal to see it). No Visual C++ redistributable is needed.
2. Unsigned builds: SmartScreen shows "Windows protected your PC"; choose **More info → Run
   anyway**.

`ac2-<version>-windows-x64.zip` holds the same three programs for use without installing.
Uninstall from **Settings → Apps** like any other program.

On Windows the local daemon listens on `tcp://127.0.0.1:47820` and `:47821` (loopback only).
WASAPI needs a buffer of 256 frames or more for reliable duplex; set it per session with
`--buffer 256samples`.

## Starting a daemon

There are three ways; pick one.

- **The app hosts it.** Start **ac2**. If no daemon is running, the connect dialog opens:
  choose *This computer's audio* (the session dialog opens next, to pick the interface and
  channels) or *Simulated rig* to try ac2 without hardware (it starts with its session open
  and a transfer measurement "demo" running). That daemon lives inside the app and stops
  with it. The command-line client cannot reach it; everything it would do is in the app.
- **A per-user daemon** that the app and the CLI share: `systemctl --user enable --now ac2d`
  (Linux), or `ac2 daemon start` on any OS (`ac2 daemon stop` stops it). The app connects to
  it automatically.
- **A network daemon** on a stage or FOH machine, used from another computer. See
  [Remote use](#remote-use-foh--stage).

## First measurement

A transfer function compares a **measurement** input (the mic) with a **reference** input
(the signal you send to the system, looped back from your interface's output). Wire:

```
interface out 1 ──┬──► system under test (amp / processor / speaker) … mic ──► in 2
                  └──► loopback cable ──────────────────────────────────────► in 1
```

**In the app** (any daemon: hosted by the app, per-user, or remote; no terminal needed):

1. Until there is an audio session the transfer pane says *No audio session — press
   Shift+O*. **Shift+O** (or **Ctrl+K** → *Open audio session…*) opens the session dialog:
   it lists the daemon's interfaces. **↑/↓** move between fields, **←/→** pick the backend
   and device, type the input channels (`1-2`), the number of output channels, optionally a
   rate and buffer (empty: the device's defaults) and the loopback (`1>1`: output 1 returns
   on input 1; leave it empty if your interface does not loop the output back). **Enter**
   opens it.
2. The pane now says *No measurements*. **Ctrl+K**, type `new transfer`, **Enter**: the
   dialog proposes the loopback input as reference and the next input as measurement
   (**Enter** creates and starts it; it is selected). *New spectrum…*, *New RTA…* and *New
   SPL meter…* work the same way; *Delete selected measurement* and *Close audio session*
   are in the palette too.

**Or from a terminal**, with a per-user daemon running (see above):

```sh
ac2 devices                                       # find your interface
ac2 session open --backend cpal --device "<name>" --in 1-2
ac2 meas new tf --ref 1 --meas 2 --name main
ac2 meas start main
ac2-ui                                            # or start ac2 from the menu
```

The app's dialogs use the same defaults as these commands. Then in the app (or with
`ac2 gen pink --out 1 --level -30dbfs`, which runs in the foreground: **Enter** fires,
**Esc** stops):

1. Press **L**, type a level such as `-30` (dBFS) and Enter. ac2 never plays anything at a
   default level.
2. Press **Space** to arm and **Enter** to fire: pink noise plays on output 1.
3. Press **X** to find the delay and insert it. The phase trace flattens; coherence (the
   transparency of the trace) shows where the data is trustworthy.
4. **Esc** stops the noise at any time.

No hardware at hand? Use the built-in simulated rig (output 1 → input 1 loopback, output 1
→ input 2 through a speaker-and-room model; it never touches real audio). In the app, choose
*Simulated rig* in the connect dialog: its session is already open (inputs 1–2, output 1,
loopback 1 → 1) and the transfer measurement "demo" (reference 1, measurement 2) is running,
so steps 1–4 above work at once. From a terminal:

```sh
ac2d --backend fake &
ac2 session open --backend fake --in 1-2
ac2 meas new tf --ref 1 --meas 2 --name demo && ac2 meas start demo
ac2-ui
```

### Timing

`tools/release/first-measurement.sh <tarball>` replays the Linux path in a throw-away home
directory against the simulated rig and prints a timestamp per step. On the release
tarball: unpacked and installed after 0.3 s, daemon up after 0.3 s, session and running
transfer measurement after 0.4 s, delay found and inserted and a first trace captured after
6.4 s (4 s of that is the noise averaging before the finder runs). The rest of the
two-minute budget is the person: downloading, typing five commands (or, in the app, **L**, a
level, **Space**, **Enter**, **X**) and wiring the loopback cable. On macOS and Windows the
installer replaces `install.sh`; the CI release job runs the same commands on each OS after
installing the artifact.

## Remote use (FOH ↔ stage)

The daemon runs next to the audio interface; the app or CLI runs anywhere on the network.
Network mode always encrypts and authenticates both sockets (CURVE); a client must be paired
before it can do anything.

On the daemon host:

```sh
ac2d --listen tcp://0.0.0.0 --name "FOH rack"
```

It logs its key and fingerprint:

```
network mode: 0 authorized client(s); server key rq:A1…(40 characters) (fingerprint 1a2b-3c4d-5e6f-7a8b-9c0d)
```

and advertises itself on the local network over mDNS (`--no-mdns` turns that off). Open
TCP ports 47820 and 47821 in its firewall.

On the client:

```sh
ac2 discover                                       # lists rigs, their fingerprints and pairing
ac2 auth pair 10.0.0.20 --server-key '<the 40-character key from the daemon host>'
```

`auth pair` pins the daemon key and prints this client's key as one line. Add that line to
the daemon host's `authorized_clients` file and restart the daemon. Then
`ac2 --remote 10.0.0.20 status`, or open the app's connect dialog (`ac2-ui --connect`): paired
rigs are selectable there, and an unpaired rig offers the same pairing steps.

Where the keys live (the ac2 config directory, shared by every ac2 program on a machine):

| file | Linux | macOS | Windows |
|---|---|---|---|
| daemon key pair `server.key`, accepted clients `authorized_clients` | `~/.config/ac2/` | `~/Library/Application Support/ac2/` | `%APPDATA%\ac2\config\` |
| this client's key pair and pinned daemon keys (`keys/client.key`, `keys/client.pub`, `keys/known_servers`) | `~/.config/ac2/keys/` | `~/Library/Application Support/ac2/keys/` | `%APPDATA%\ac2\config\keys\` |

`AC2_CONFIG_DIR` moves the whole directory; `ac2d --key-file` / `--authorized` and
`ac2 --key-dir` (or `AC2_KEY_DIR`) override single locations. `ac2 auth show` prints the
client side. The app's connect dialog pairs into the same `keys/` directory as the CLI, so a
rig paired with either is paired for both.

Discovery is only a convenience. Anyone on the network can advertise any name and any
fingerprint; ac2 connects only with a key you pinned, and the connection fails if the daemon
does not hold that key. Compare the fingerprint the client shows with the one the daemon host
shows before pairing. mDNS does not cross routers or VPNs: there, use the address directly.
