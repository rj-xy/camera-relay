# Laptop camera

The built-in webcam of a Dell XPS 14 DA14260 (Panther Lake). It didn't work on Kubuntu 26.04 at first; it does since 2026-11-04, through a relay that browsers and call apps see as **Built-in Camera** (2026-11-04).

| | |
| --- | --- |
| 🟢 Status | Works in Brave and Edge: upright, neutral colour, 1280×720 at 29 fps on webcamtests.com (2026-11-04). Grainy in office light (high gain, see below) |
| Turn it on | KDE menu **Camera Relay (on/off)**, or `camera-relay toggle` (`start`, `stop`, `status`). A notification says on or off. Turn it off after the call |
| While it's on | The camera's privacy LED is on, and the relay uses about 56 % of one CPU core (measured 2026-11-04) |
| Apps see | **Built-in Camera**, 1280×720 YUY2 at 30 fps: a PipeWire camera, only while the relay runs. Apps that read `/dev/video*` directly (not through PipeWire) don't see it |
| Hardware | OmniVision **ov08x40** (8 MP, ACPI `OVTI08F4`) on Intel **IPU7** (`intel_ipu7`, `intel_ipu7_isys`). The sensor sits behind Intel's **CVS** chip (ACPI `INTC10E1`, `\_SB_.PC00.CVSS`); its power GPIOs are on the **Synaptics SVP7500** USB-IO bridge (`06cb:0701`, `usbio`, `gpio_usbio`, ACPI `INTC10E2`) |
| Kernel | 7.0.0-42-generic, BIOS 1.12.0, Secure Boot off (2026-11-04) |
| IR camera | The Windows Hello IR sensor (Himax HM1092) has no Linux driver; not set up |

## Test results

<https://webcamtests.com> in Edge, office light (2026-11-04). Before tuning: relay without [ov08x40.yaml](ov08x40.yaml) (the run that showed upside down). After: with it.

| Stat | Before tuning | After tuning |
| --- | --- | --- |
| Webcam name | Built-in Camera | Built-in Camera |
| Quality rating | 1825 | 60 |
| Frame rate | 29 FPS | 29 FPS |
| Stream type, image mode | video, rgb | video, rgb |
| Resolution | 1280×720 (0.92 MP, HD, 1.78) | 1280×720 (0.92 MP, HD, 1.78) |
| PNG / JPEG file size | 1.9 MB / 935.05 kB | 1.9 MB / 959.57 kB |
| Bitrate | 26.29 MB/s | 26.99 MB/s |
| Number of colours | 125930 | 162194 |
| Lightness | 49.80 % | 49.22 % |
| Luminosity | 55.73 % | 49.10 % |
| Brightness | 51.11 % | 49.15 % |
| Hue | 🔴 75° (yellow-green) | 🟢 0° |
| Saturation | 🔴 15.75 % | 🟢 0.40 % (neutral grey average) |
| Microphone, speaker | None | None |

- Hue and saturation are of the picture's average colour: after tuning it's a neutral grey, so the cast is gone.
- **Re-test after the 2026-11-05 reboot (09:03 AEST):** works. Home office lit only by two LED downlights, so dim. Edge, `Built-in Camera`, 1280×720, 29 FPS, quality rating 1003, lightness 51.37 %, luminosity 48.84 %, brightness 50.59 %, hue 248°, saturation 6.45 %. Brightness is right even in the dim room (auto-exposure copes). The average colour is a slight blue cast (6 % saturation, against 0.40 % in the office light): the wall and the picture look a little cool and grainy. The tuning was done by eye in the office, so this is probably the LED downlights' colour plus the extra gain, not a fault. Not retuned.
- The site doesn't explain its quality rating; it dropped from 1825 to 60 with the better picture, so it's not a useful measure here.

## How it works

```
ov08x40 (3856×2176 mode only) → IPU7 ISYS → libcamera (simple pipeline, GPU soft-ISP) at 2560×1440
  → gst-launch-1.0 scales to 1280×720 YUY2 → pipewiresink: PipeWire camera "Built-in Camera"
  → browsers through the camera portal
```

- [camera-relay.service](camera-relay.service): systemd user unit running the GStreamer pipeline. No `[Install]` section, so it only runs when turned on. Installed copy: `~/.config/systemd/user/`.
- [camera-relay.sh](camera-relay.sh): the toggle, installed as `~/.local/bin/camera-relay`. [camera-relay.desktop](camera-relay.desktop) is its KDE menu entry.
- [ov08x40.yaml](ov08x40.yaml): libcamera soft-ISP tuning, installed to `~/.config/camera-relay/` and passed to the relay as `LIBCAMERA_SIMPLE_TUNING_FILE`. Without it (`uncalibrated.yaml`) the picture was yellow-green: blue about 13 % low, webcamtests hue 75° (2026-11-04). It sets the sensor's black level (64 at 10 bit), a colour matrix (green × 0.92, blue × 1.28) and, with the relay's `saturation=1.3`, more colour. Tuned by eye in the office, so other light may need other numbers. The relay ignores manual exposure, gain and colour gains (`libcamerasrc` properties tested 2026-11-04); contrast and saturation work.
- [51-camera-relay.conf](51-camera-relay.conf): WirePlumber rules in `~/.config/wireplumber/wireplumber.conf.d/`. They turn off PipeWire's libcamera camera (it crashes, see below) and hide the 32 raw `ipu7` capture devices, which never give a picture.
- The relay offers PipeWire one fixed format (1280×720 at 30 fps). Edge and Brave skip any PipeWire camera that only lists size ranges, with `NotFoundError: Requested device not found` (found 2026-11-04 11:22 AEST). That's why the first version, which used `v4l2loopback` `/dev/video60`, didn't work: PipeWire read that device's formats at boot, before the relay ran, and saw only ranges. Since 11:40 AEST the relay uses `pipewiresink mode=provide` and the loopback is gone.
- **Browsers** need PipeWire camera support on, or they open the raw `ipu7` devices and get nothing:
  - Brave: `brave://flags/#enable-webrtc-pipewire-camera` set to **Enabled** (2026-11-04).
  - The browser must be fully restarted after PipeWire restarts or crashes, or it keeps a dead camera (it showed the old **Built-in Front Camera** on 2026-11-04).
  - Edge: the flag doesn't show on `edge://flags`, so [install.sh](install.sh) writes `~/.local/share/applications/microsoft-edge.desktop`, a copy of Edge's launcher with `--enable-features=WebRtcPipeWireCamera`. It only applies when Edge starts fresh: quit it with **⋯ → Close Microsoft Edge** first. Starting `microsoft-edge --enable-features=…` while Edge runs only opens a window in the running Edge, without the option.
  - KDE asks **Allow Apps to Use the Camera?** the first time; click OK.

## Why a relay

PipeWire's own libcamera camera works at full size, but any app asking for 1920×1080 or less crashes PipeWire. Found 2026-11-04 (Edge on webcamtests.com, 10:58 AEST, Apport showed `/usr/bin/pipewire`):

- For sizes up to 1920×1080, libcamera switches the sensor to its binned **1928×1088** mode. In that mode the raw frames are bad: 2,121,824 of 2,123,776 pixel values were over 1023, the most a 10-bit pixel can hold (max 65535). The full **3856×2176** mode is clean (max 1023).
- The bad values overflow the 64-bin histogram in libcamera's `SwStatsCpu::processBayerFrame2`, which aborts on Ubuntu's hardened build (`std::array<unsigned int, 64>::operator[]` assertion). CPU mode (`LIBCAMERA_SOFTISP_MODE=cpu`) crashes the same way.
- Tested with `cam`: 640×480, 1280×720, 1920×1080, 1924×1088 crash; 2560×1440, 3840×2160, 3848×2176 work.
- Browsers ask for 640×480 or 1280×720, so they always hit it. The relay always reads 2560×1440 (sensor in the full mode) and scales down itself.
- Most likely a bug in the kernel `ov08x40` driver's binned mode, not reported anywhere yet (searched 2026-11-04). When a kernel or libcamera update fixes it, the relay can go: delete the WirePlumber rule and PipeWire's libcamera camera comes back.

## What was wrong (2026-11-04)

1. 🔴 **No `intel_cvs` driver.** The sensor depends on the CVS chip in ACPI, so without the driver the kernel never created the sensor (`ipu7` found `OVTI08F4:00`, but no `ov08x40` in the media graph, `cam -l`: `No sensor found for /dev/media0`). Fix: Ubuntu's signed `linux-modules-vision-generic` (`intel_cvs.ko`, matches `INTC10E1`). After the reboot at 10:46 AEST: `Transfer of ownership success`, then `bind ov08x40 13-0036 nlanes is 2 port is 0`.
   - The `int3472-discrete … cannot find GPIO chip INTC10E2:00, deferring` lines at boot are harmless: `INTC10E2` is the USB-IO bridge's GPIO, it shows up a moment later and `INT3472` binds. Claude first blamed these, wrongly, and found the real cause the same morning.
   - Neither Ubuntu's generic nor OEM kernel (`linux-oem-26.04`, 7.0.0-1017) has a pinctrl driver for `INTC10E2`; it doesn't need one.
2. 🔴 **No camera software.** I installed `libcamera-tools`, `libspa-0.2-libcamera`, `gstreamer1.0-libcamera` (2026-11-04).
3. 🔴 **Browsers read the raw devices.** Fixed with the PipeWire camera flag (above).
4. 🔴 **PipeWire crash below 2560×1440.** Fixed with the relay (above).

- `intel-ipu7-dkms` (suggested by `ubuntu-drivers`): I installed it, then removed it the same day. It only adds `intel-ipu7-psys` and `ipu-acpi*`, used by Intel's own camera stack (not installed), and a CachyOS guide had to blacklist its `psys` module because it clashed with the kernel's driver.
- Kernel 7.2 (has `intel_cvs` built in): not in Ubuntu's apt for 26.04 (all 7.0, checked 2026-11-04). Mainline builds exist but lose Ubuntu's `vision`/`usbio` modules, and an XPS 14 DA14260 lost its sensor on 7.2.3 ([omarchy #10948](https://github.com/omacom/omarchy/issues/10948)). Not tried.

## Set it up again

1. Root part (packages, CVS driver; removes the old `/dev/video60` loopback), in your terminal. Asks for the sudo password, ends with 🟢 SUCCESS:

   ```bash
   bash ~/camera-relay/legacy-relay/setup-root.sh
   ```

2. Reboot if it says `intel_cvs not loaded yet`. Check that the sensor is there (`1: Internal front camera`; with the relay off, it holds the camera):

   ```bash
   cam -l
   ```

3. User part (relay, toggle, menu entry, WirePlumber rules, Edge launcher):

   ```bash
   bash ~/camera-relay/legacy-relay/install.sh
   ```

4. Brave: enable `brave://flags/#enable-webrtc-pipewire-camera`. Test with `camera-relay start`, then <https://webcamtests.com>.

## Watch out for

- 🟠 **BIOS 1.12.0:** another XPS 14 DA14260 on this BIOS gets no frames at all (CVS firmware ≥ 1.12, [omarchy #16792](https://github.com/omacom/omarchy/issues/16792)); BIOS 1.11 was suggested there. Ours streams fine on 1.12.0. Check the camera after any BIOS update.
- 🟠 **Suspend:** on the XPS 16 the sensor bind is lost after suspend ([omarchy #10624](https://github.com/omacom/omarchy/issues/10624)); the USB-IO bridge can also hang on disconnect and break suspend ([omarchy #15693](https://github.com/omacom/omarchy/issues/15693)). Not seen here yet. If the camera is gone after resume, reboot.
- 🪵 `intel_cvs` logs a kernel `WARNING` in `__setup_irq` while loading (2026-11-04). The driver still works.
- 🪵 **Apport pop-up "Ubuntu 26.04 has experienced an internal error", `/usr/bin/cam`, at login (2026-11-05 08:59 AEST):** a stale report, not a new crash. It's the deliberate `cam -c1 --capture=3 -s width=1280,height=720` test from 2026-11-04 11:00 AEST (SIGABRT, the binned-mode crash under "Why a relay"). Apport re-showed it about 8 minutes after boot. That boot was clean: `intel_cvs` loaded, `ov08x40` bound, the relay was off. The same test left `pipewire` and `timeout` reports in `/var/crash/` too. Fix: delete the three `_usr_bin_cam`, `_usr_bin_pipewire` and `_usr_lib_cargo_bin_coreutils_timeout` `.crash` and `.upload` files. A real new crash shows a different `Date:` and `ProcCmdline` (`grep -a '^Date\|^ProcCmdline' /var/crash/*.crash`).
- 🪵 **Grain:** in office light auto-exposure runs the sensor near maximum exposure and about 10× analogue gain, so the picture is noisy (I called it flicker). The overall brightness is steady and there's no mains banding (measured 2026-11-04). The full-resolution mode needs that gain; the binned mode, which collects more light per pixel, is the broken one.
- 🪵 **Orientation:** the sensor is mounted rotated 180°; libcamera fixes that by setting the sensor's flip bits, so no `videoflip` is needed. I once saw the picture upside down in Edge (2026-11-04). It was upright after a relay restart, and the cause wasn't found. If it happens again, restart the relay (`camera-relay stop`, then `camera-relay start`).
- 🪵 libcamera warns `Failed to create camera sensor helper for ov08x40`: no gain model for this sensor, which may be why manual exposure and gain do nothing.
- 🪵 Logs: `journalctl _SYSTEMD_USER_UNIT=camera-relay.service` (`journalctl --user -u` shows nothing on this laptop).

## Undo

```bash
camera-relay stop; rm ~/.config/systemd/user/camera-relay.service ~/.config/wireplumber/wireplumber.conf.d/51-camera-relay.conf ~/.local/bin/camera-relay ~/.local/share/applications/camera-relay.desktop ~/.local/share/applications/microsoft-edge.desktop; systemctl --user daemon-reload; systemctl --user restart wireplumber
```

## Sources

- [omarchy #10948](https://github.com/omacom/omarchy/issues/10948): ov08x40 missing from the media graph on the XPS 14 DA14260
- [omarchy #16792](https://github.com/omacom/omarchy/issues/16792): XPS 14 DA14260, BIOS 1.12.0, no frames
- [intel/vision-drivers #36](https://github.com/intel/vision-drivers/issues/36): `intel_cvs` needed for IPU7 on Panther Lake
- [CachyOS #804](https://github.com/CachyOS/linux-cachyos/issues/804): IPU7 camera on Panther Lake (Dell XPS) guide
- [Ubuntu package intel-vision-dkms](https://packages.ubuntu.com/eu/resolute/kernel/intel-vision-dkms)
- [Debian bug #1147085](http://www.mail-archive.com/debian-bugs-dist@lists.debian.org/msg2121007.html): CVS driver merged in Linux 7.2
