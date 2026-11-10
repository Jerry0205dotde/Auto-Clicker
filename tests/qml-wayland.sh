#!/usr/bin/env bash
# Run against an isolated KWin compositor; never change the running desktop.
set -euo pipefail

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
qml_runner="${QMLTESTRUNNER:-/usr/lib/qt6/bin/qmltestrunner}"
for dependency in kwin_wayland kscreen-doctor dbus-run-session setsid timeout python3 "$qml_runner"; do
  command -v "$dependency" >/dev/null || { printf 'Missing dependency: %s\n' "$dependency" >&2; exit 1; }
done
# KWin 6.8 composites only with OpenGL. Without a render device it starts
# without compositing, and window tests fail later with unrelated symptoms.
if [[ ! -e /dev/udmabuf ]] && ! compgen -G '/dev/dri/renderD*' >/dev/null; then
  printf 'Missing render device: KWin needs /dev/udmabuf or /dev/dri/renderD*\n' >&2
  exit 1
fi

test_dir="$(mktemp -d)"
session_pid=""
cleanup() {
  # Portal backends activated by the private bus can outlive KWin and the bus.
  # Keep the entire test session in its own process group and reap it on exit.
  if [[ -n "$session_pid" ]]; then
    kill -TERM -- "-$session_pid" 2>/dev/null || true
    wait "$session_pid" 2>/dev/null || true
    for ((attempt = 0; attempt < 20; ++attempt)); do
      if ! kill -0 -- "-$session_pid" 2>/dev/null; then break; fi
      sleep 0.05
    done
    kill -KILL -- "-$session_pid" 2>/dev/null || true
  fi
  # A document portal can release its FUSE mount asynchronously after exit.
  for ((attempt = 0; attempt < 20; ++attempt)); do
    if rm -rf -- "$test_dir" 2>/dev/null; then return; fi
    sleep 0.05
  done
  rm -rf -- "$test_dir"
}
trap cleanup EXIT
mkdir -m 700 "$test_dir/runtime" "$test_dir/config"
export PICKER_TEST_DIR="$test_dir" PICKER_TEST_REPO="$repo_dir" PICKER_TEST_RUNNER="$qml_runner"
cat > "$test_dir/run.sh" <<'SESSION'
#!/usr/bin/env bash
set -euo pipefail
export QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software QT_QUICK_CONTROLS_STYLE=org.kde.desktop
scale="${KLICKMEISTER_TEST_SCALE:-1}"
# KWin rejects negative output positions. Translate the staggered layout so
# Virtual-2 remains above and left of Virtual-0 without a negative origin.
kscreen-doctor "output.Virtual-0.scale.$scale" \
  "output.Virtual-1.scale.${KLICKMEISTER_TEST_SCALE_1:-$scale}" \
  "output.Virtual-2.scale.${KLICKMEISTER_TEST_SCALE_2:-$scale}" \
  output.Virtual-0.position.1920,1080 output.Virtual-1.position.3840,1080 output.Virtual-2.position.0,0 > "$PICKER_TEST_DIR/outputs.log" 2>&1
kscreen-doctor -o >> "$PICKER_TEST_DIR/outputs.log" 2>&1
# kscreen-doctor can return success even when KWin rejects the configuration.
# Verify the effective state before running any QML assertions.
kscreen-doctor -j > "$PICKER_TEST_DIR/outputs.json" 2>> "$PICKER_TEST_DIR/outputs.log"
python3 "$PICKER_TEST_REPO/tests/check_virtual_outputs.py" "$PICKER_TEST_DIR/outputs.json" \
  "$scale" "${KLICKMEISTER_TEST_SCALE_1:-$scale}" "${KLICKMEISTER_TEST_SCALE_2:-$scale}" > "$PICKER_TEST_DIR/verified.log" 2>> "$PICKER_TEST_DIR/outputs.log"
"$PICKER_TEST_RUNNER" -import "$PICKER_TEST_REPO/tests/qml/mocks" -input "$PICKER_TEST_REPO/tests/qml" > "$PICKER_TEST_DIR/tests.log" 2>&1
SESSION
chmod +x "$test_dir/run.sh"
status=0
setsid timeout --foreground 60s env \
  XDG_RUNTIME_DIR="$test_dir/runtime" XDG_CONFIG_HOME="$test_dir/config" \
  QT_QPA_PLATFORM=offscreen KWIN_COMPOSE=O2 \
  dbus-run-session -- \
  kwin_wayland --virtual --width 1920 --height 1080 --scale "${KLICKMEISTER_TEST_SCALE:-1}" --output-count 3 \
  --no-lockscreen --no-global-shortcuts --no-kactivities \
  --exit-with-session "$test_dir/run.sh" > "$test_dir/kwin.log" 2>&1 &
session_pid=$!
wait "$session_pid" || status=$?
if [[ -f "$test_dir/verified.log" ]]; then cat "$test_dir/verified.log"; fi
if [[ -f "$test_dir/tests.log" ]]; then cat "$test_dir/tests.log"; fi
if ((status != 0)); then
  cat "$test_dir/kwin.log" >&2
  if [[ -f "$test_dir/outputs.log" ]]; then cat "$test_dir/outputs.log" >&2; fi
fi
exit "$status"
