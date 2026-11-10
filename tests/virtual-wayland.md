# Virtual Wayland test environment

`bash tests/qml-wayland.sh` starts a private D-Bus session and a virtual KWin
with three outputs, including an output above and left of the others. It runs the
complete QML suite using real Wayland windows. The script isolates runtime and
configuration directories and cleans up the entire session process group.
Python 3 is required to verify the effective output configuration before QML runs.

The `virtual-wayland` CI job installs rolling Arch Linux packages, like the
other jobs, and prints the installed KWin, Qt, libkscreen, and Mesa versions.
The container receives the runner's `/dev/udmabuf` and nothing else from the host.

## Why the render device is required

KWin 6.8 removed the virtual backend's QPainter renderer. Compare
[6.7.5's supported compositors](https://github.com/KDE/kwin/blob/v6.7.5/src/backends/virtual/virtual_backend.cpp#L89-L106)
with [6.8.0's OpenGL-only backend](https://github.com/KDE/kwin/blob/v6.8.0/src/backends/virtual/virtual_backend.cpp#L37-L44).
OpenGL needs a render device: KWin's
[GPU manager](https://github.com/KDE/kwin/blob/v6.8.0/src/core/gpumanager.cpp#L80-L99)
uses `/dev/udmabuf` for a software device or an accessible DRM device.
`udmabuf` allocates buffers from memory and does not require a GPU.

Without either device, KWin 6.8.0 and 6.8.1 crash during startup. Since
[6.8.2](https://github.com/KDE/kwin/commit/e749ee8617) it starts without
compositing instead. The QML suite then fails in window tests, for example in
`test_restores_window_geometry`, and the window of `tst_positionpicker.qml` is
never exposed. `tests/qml-wayland.sh` therefore refuses to start without
`/dev/udmabuf` or a `/dev/dri/renderD*` node and requests OpenGL compositing with
`KWIN_COMPOSE=O2`.

The old script also requested a negative position for `Virtual-2`. KWin rejects
that request, while `kscreen-doctor` returns zero with an error message. The
entire configuration, including scale changes, then remains unapplied. The
layout is now translated 1920 logical pixels to the right: `Virtual-0` at
`1920,1080`, `Virtual-1` at `3840,1080`, and `Virtual-2` at `0,0`. This preserves
their relative positions. `check_virtual_outputs.py` checks KScreen's JSON for
exactly those three connected, enabled outputs, their positions, and all
requested scales. A mismatch fails the session before any QML tests run.

## Reproduce CI locally

The host needs `/dev/udmabuf`. If it is missing, load the module with
`sudo modprobe udmabuf`. Build a disposable image with this
Dockerfile, keeping the package list in sync with `.github/workflows/ci.yml`:

```dockerfile
FROM archlinux:base-devel
RUN pacman -Syu --noconfirm --needed \
      git ca-certificates python qt6-base qt6-declarative qt6-wayland \
      kirigami qqc2-desktop-style kwin libkscreen dbus mesa libcap \
 && setcap -r /usr/bin/kwin_wayland
WORKDIR /work
```

With the resulting image tagged `klickmeister-virtual-wayland:local`, run from
the checkout root:

```bash
docker run --rm --network none --device /dev/udmabuf \
  --mount "type=bind,source=$PWD,target=/work,readonly" \
  klickmeister-virtual-wayland:local bash tests/qml-wayland.sh
```

Additional runs can pass `-e KLICKMEISTER_TEST_SCALE=1.5`, or
`-e KLICKMEISTER_TEST_SCALE_1=1.5 -e KLICKMEISTER_TEST_SCALE_2=2`, to Docker.
The runtime container needs no network access, no GPU, and no mounts from a
desktop session. The session stops after 60 seconds; on CI the suite needs about
20 seconds, but an older laptop such as an i5-7200U needs longer at every scale.

## Updating the environment

The job follows rolling Arch packages. Install the complete dependency set from
the same repositories in a fresh container; do not pin KWin alone. If a new KWin
changes its render device requirements, adapt the device given to the container
and the check in `tests/qml-wayland.sh` together with this document. Keep the
isolated session, the three-output layout, and the full QML assertions, and
require the complete suite to pass at scale 1, scale 1.5, and mixed scales.
A startup crash remains a failing test, including when it produces no QML log.
