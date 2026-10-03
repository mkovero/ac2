# Installing ac2

ac2 ships three programs:

| program | what it is |
|---|---|
| `ac2-ui` | the desktop app (shown as **ac2** in menus). It can host its own daemon. |
| `ac2d` | the daemon: owns the audio interface, runs the measurements. |
| `ac2` | the command-line client: set up sessions and measurements, script everything. |

There is no published GitHub release yet. The installers come from the
[Release workflow](https://github.com/mkovero/ac2/actions/workflows/release.yml): open its
latest successful run (**Actions → Release**, signed in to GitHub) and download an artifact
from the run's **Artifacts** list — `dist-linux`, `dist-macos`, `dist-windows`, or
`release-<version>` with all of them and a `SHA256SUMS` file. A maintainer starts a new build
with `gh workflow run release.yml`; a `v*` tag makes the same build into a draft release.
Check a download with `sha256sum -c SHA256SUMS --ignore-missing` (Linux),
`shasum -a 256 -c SHA256SUMS --ignore-missing` (macOS) or `Get-FileHash <file>` (Windows
PowerShell). The installers are not code-signed yet; the macOS and Windows sections say how
to open them anyway. To build from source instead, see the
[README](../README.md#building-from-source).

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

**Audio on Linux is JACK**: either a JACK2 server, or PipeWire through its JACK library
(pipewire-jack). There is no ALSA backend: an ALSA device left to choose its own buffer
under PipeWire delivered audio in 1.4-second lumps, and every meter went stale between
them. JACK gives every port one clock, exact sample indices and the server's short period.

- **PipeWire desktops** (Ubuntu 22.10+, Fedora, Debian 12+, Arch with PipeWire): install
  PipeWire's JACK library, then nothing else is needed:
  ```sh
  sudo apt install pipewire-jack          # Debian / Ubuntu
  sudo dnf install pipewire-jack-audio-connection-kit   # Fedora
  sudo pacman -S pipewire-jack            # Arch
  ```
  On Debian and Ubuntu the package does not replace the system libjack; either run the
  daemon through it (`pw-jack ac2d`, `pw-jack ac2-ui --embedded`) or make it the system's
  libjack once:
  `sudo cp /usr/share/doc/pipewire/examples/ld.so.conf.d/pipewire-jack-*.conf /etc/ld.so.conf.d/ && sudo ldconfig`.
- **JACK2**: start the server on your interface before the daemon, e.g.
  `jackd -d alsa -d hw:UMC1820 -r 48000 -p 256` (or with QjackCtl).

The daemon connects JACK ports itself. The session's inputs come from the capture ports of
the same number (input 1 = the first capture port in the device list). Outputs are
connected only when you choose them for the stimulus: arming the generator on `--out 1,2`
connects `ac2:out_1` and `ac2:out_2` to the first and second playback ports, and the
session's loopback output is connected when the session opens. Other outputs are never
connected, connections the daemon did not make (a recorder, a patch to another program)
are left alone, and changing the generator's outputs never reopens the stream, so they
stay.

When JACK cannot be used, `ac2 devices`, the session dialog and the daemon log say why and
what to do: *PipeWire is running but its JACK library isn't in use: install pipewire-jack …
or start the daemon with `pw-jack ac2d`*, or *No JACK server: start JACK (e.g.
`jackd -d alsa`) or use PipeWire*. libjack is loaded at run time, so ac2 starts either way.

**AppImage** (the desktop app only, with its embedded daemon):

```sh
chmod +x ac2-ui-<version>-x86_64.AppImage
./ac2-ui-<version>-x86_64.AppImage
```

The AppImage bundles no system libraries; it needs glibc and libstdc++, which every desktop
has. It starts without JACK (the simulated rig works anywhere); to measure real audio it loads
the system's `libjack.so.0` (pipewire-jack or JACK2, above) at run time. Without FUSE, run it
with `--appimage-extract-and-run`.

Real-time scheduling: the audio thread asks for real-time priority. With PipeWire this is
granted through rtkit; with JACK2 add yourself to the `audio` (or `realtime`) group your
distribution configures in `/etc/security/limits.d`.

## macOS (11 Big Sur or newer, Apple silicon and Intel)

1. Open `ac2-<version>-macos-universal.dmg` and drag **ac2** to Applications.
2. Optional, for the command-line tools: copy `bin/ac2` and `bin/ac2d` from the disk image
   to a directory on your `PATH`:
   ```sh
   sudo cp /Volumes/ac2*/bin/ac2 /Volumes/ac2*/bin/ac2d /usr/local/bin/
   ```
3. Start **ac2** from Applications. macOS asks for microphone access the first time an input
   is opened; allow it (ac2 reads your audio interface's inputs, nothing else).

Unsigned builds (every build so far: the project has no Developer ID yet): macOS refuses
to open them at first. Open **System Settings → Privacy & Security** and click **Open
Anyway** for ac2, or run `xattr -dr com.apple.quarantine /Applications/ac2.app`. For the CLI
tools: `xattr -d com.apple.quarantine /usr/local/bin/ac2 /usr/local/bin/ac2d`.

The daemon can run as a launchd agent: `launchd/io.github.mkovero.ac2d.plist` in the disk
image has the instructions in its header. A daemon started by launchd cannot show the
microphone prompt, so either let the app host its daemon or start `ac2d` from Terminal.

macOS builds are compiled and tested in CI but not yet verified with a real audio interface.

## Windows (10 1809 or newer, x64)

1. Run `ac2-<version>-windows-x64.msi`. It installs into `C:\Program Files\ac2`, adds a
   Start-menu entry **ac2**, and puts the install folder on the system `PATH` (open a new
   terminal to see it). No Visual C++ redistributable is needed.
2. The installer is not code-signed yet: SmartScreen shows "Windows protected your PC";
   choose **More info → Run anyway**.

`ac2-<version>-windows-x64.zip` holds the same three programs for use without installing.
Uninstall from **Settings → Apps** like any other program.

On Windows the local daemon listens on `tcp://127.0.0.1:47820` and `:47821` (loopback only).
WASAPI needs a buffer of 256 frames or more for reliable duplex; set it per session with
`--buffer 256samples`. So far the MSI install and the simulated rig are verified on Windows;
WASAPI with a real interface is not yet.

## Starting a daemon

There are three ways; pick one.

- **The app hosts it.** Start **ac2**. If no daemon is running, the connect dialog opens:
  choose *This computer's audio* (the session dialog opens next, to pick the interface and
  channels) or *Simulated rig* to try ac2 without hardware (it starts with its session open
  and a transfer measurement "demo" running). That daemon lives inside the app and stops
  with it. The command-line client cannot reach it; everything it would do is in the app.
- **A per-user daemon** that the app and the CLI share: `systemctl --user enable --now ac2d`
  (Linux), or `ac2 daemon start` on any OS (`ac2 daemon stop` stops it). The app connects to
  it automatically. It autosaves measurements and traces and restores them, disarmed, when it
  restarts; the top bar says *autosaved just now* (or why it failed). `ac2d --no-restore`
  starts empty, `--autosave <dir>` keeps the autosave elsewhere, `--no-autosave` keeps
  everything in memory only ([user guide](user-guide.md#autosave)).
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
   Shift+O*. **Shift+O** (or **Ctrl+K** → *Open audio session…*) opens the session dialog.
   The top rows pick the **backend** (JACK on Linux, system audio on macOS and Windows; one
   that cannot be used says why and what to do, e.g. *No JACK server: start JACK (e.g.
   `jackd -d alsa`) or use PipeWire*) and the **device** (*8 in / 8 out · 48 kHz*) with
   **←/→**. Below them is one row per input and output with its name and, for inputs, a
   live level meter — tap the mic or play something and you see which input it is on,
   before anything is opened (the dialog only listens; it never plays).
2. Mark what each channel is for, with **↑/↓** to the row and one key:
   - **R** on the input the loopback cable returns on: the **Reference**;
   - **M** on each input with a measurement mic; **N** types its name (`M30 FOH`), which
     is what calibrations are kept under;
   - **S** on the output that feeds the system and the loopback: the **Stimulus**.

   **Space** adds or removes a row from the session. Not sure which input the loopback
   is on? **D** (*Detect loopback…*) plays a 0.5 s noise burst on the stimulus output at a
   level you type (there is no default level; **Enter** plays) and marks the input it
   comes back on as the Reference. The dialog says in words what is missing (*Pick a
   reference input: the loopback from your stimulus output*); rate and buffer stay at the
   device's defaults unless you type them. The roles and names are remembered per device.
3. **Enter** opens the session. With a reference and at least one mic, and no measurements
   yet, the app offers *Reference → M30 FOH* — one transfer measurement per mic; **Enter**
   creates and starts them. Later, **Ctrl+K** → *New transfer measurement…* (or *New
   spectrum…*, *New RTA…*, *New SPL meter…*) picks inputs by name with their meters
   (**←/→**); *Delete selected measurement* and *Close audio session* are in the palette
   too. While the session is open, the **Inputs** list on the left keeps a named meter per
   input with its role (*reference*, *mic*), so levels stay in sight while you measure.

**Or from a terminal**, with a per-user daemon running (see above):

```sh
ac2 devices                                       # find your interface
ac2 session open --backend jack --in 1-2                    # Linux
ac2 session open --backend cpal --device "<name>" --in 1-2  # macOS, Windows
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
With ufw, add one rule per port and limit them to your network, e.g.
`sudo ufw allow from 192.168.1.0/24 to any port 47820 proto tcp` (and 47821, and `5353/udp` for
discovery). A port *range* rule (`47820:47821`) needs the kernel's iptables `multiport` module;
without it (some real-time kernels) ufw lists the rule but does not enforce it.
The daemon names the ports at startup and warns when it sees ufw or firewalld active; a
client that gets no answer says "not responding" with the same hint.

On the client:

```sh
ac2 discover                                       # lists rigs, their fingerprints and pairing
ac2 auth pair 10.0.0.20 --server-key '<the 40-character key from the daemon host>'
```

`auth pair` pins the daemon key and prints this client's key as one line. Add that line to
the daemon host's `authorized_clients` file and restart the daemon. Then
`ac2 --remote 10.0.0.20 status`, or open the app's connect dialog (`ac2-ui --connect`): paired
rigs are selectable there, and an unpaired rig offers the same pairing steps. Quote the key:
Z85 keys can contain `-` and other shell characters, and a key that starts with `-` is still
read as the value of `--server-key`.

A client that was never paired with a host says so before it tries to connect: *not paired
with 10.0.0.20: no daemon key pinned for it in …; run `ac2 auth pair 10.0.0.20 --server-key
<the key ac2d logs at startup>` (or pair in the app's connect dialog), then authorize this
client on the daemon host*.

A client that is paired but not authorized gets no answer, because CURVE refuses it
silently. Its "not responding" message therefore also says *or this client is not authorized
on it*, with its own fingerprint and the `authorized_clients` line to add. The daemon logs
every refused key, once per key and address every 10 s:

```
refused client key fingerprint 1a2b-3c4d-5e6f-7a8b-9c0d from 10.0.0.31: not in …/authorized_clients; …
```

If that fingerprint matches what the client shows, add the key from that line to
`authorized_clients` and restart the daemon.

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
