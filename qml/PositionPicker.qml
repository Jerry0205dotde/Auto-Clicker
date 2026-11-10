import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami

Window {
    id: picker

    MonitorSelection { id: monitors }

    required property Window hostWindow
    property bool inputReady: false
    property bool selecting: false
    property bool previewOnly: false
    property bool positionPinned: false
    property int targetX: 0
    property int targetY: 0
    property int hostVisibility: Window.Windowed
    property bool hintAtBottom: false
    readonly property bool adjustable: inputReady && !previewOnly
    // Depends only on the screen, so pointer movement does not rebuild it.
    readonly property string screenName: monitors.displayName(screen, Qt.application.screens)
    signal finished()
    signal picked(int x, int y)

    function setTarget(x, y) {
        targetX = Math.max(0, Math.min(Math.round(x), Math.max(0, width - 1)))
        targetY = Math.max(0, Math.min(Math.round(y), Math.max(0, height - 1)))
        // Move the hint out of the way before the pointer reaches it.
        if (targetY < 110) hintAtBottom = true
        else if (targetY > height - 110) hintAtBottom = false
    }

    // No screenshot backdrop: the Screenshot portal returns an image URI
    // without its capture rectangle, so it cannot map to coordinates (#19).
    function begin(targetScreen, x, y, preview) {
        if (selecting || !targetScreen) return
        // Set geometry as well as screen before creating the native surface:
        // Qt Wayland uses the initial geometry to select the fullscreen output.
        screen = targetScreen
        picker.x = targetScreen.virtualX
        picker.y = targetScreen.virtualY
        picker.width = targetScreen.width
        picker.height = targetScreen.height
        hostVisibility = hostWindow.visibility
        previewOnly = preview
        positionPinned = false
        selecting = true
        inputReady = false
        targetX = x
        targetY = y
        hintAtBottom = y < 110
        showPicker()
    }

    function showPicker() {
        if (!selecting) return
        showFullScreen()
        raise()
        requestActivate()
        focusSelection()
        updateInputReady()
        if (previewOnly) previewTimer.start()
    }

    // KWin grants activation asynchronously. Minimize only after focus transfers.
    // Keep the native host surface mapped: hiding/recreating it lets Wayland
    // choose a new placement and loses the user's window geometry.
    function focusSelection() {
        if (!selecting || !visible || !active) return
        keyboard.forceActiveFocus()
        hostWindow.showMinimized()
    }

    function updateInputReady() {
        inputReady = selecting && visible && visibility === Window.FullScreen
            && width === screen.width && height === screen.height
        if (inputReady) setTarget(targetX, targetY)
    }

    function moveTarget(dx, dy) {
        if (!adjustable) return
        positionPinned = true
        setTarget(targetX + dx, targetY + dy)
    }

    function confirm() {
        if (!adjustable) return
        picked(targetX, targetY)
        finish()
    }

    function finish(restoreHost) {
        if (!selecting) return
        selecting = false
        inputReady = false
        previewTimer.stop()
        hide()
        if (restoreHost !== false) {
            if (hostVisibility === Window.Maximized) hostWindow.showMaximized()
            else if (hostVisibility === Window.FullScreen) hostWindow.showFullScreen()
            else hostWindow.showNormal()
            hostWindow.raise()
            hostWindow.requestActivate()
        }
        finished()
    }

    visible: false
    transientParent: null
    flags: Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint
    color: "transparent"
    onActiveChanged: {
        updateInputReady()
        focusSelection()
    }
    onVisibleChanged: {
        updateInputReady()
        focusSelection()
    }
    onScreenChanged: updateInputReady()
    onVisibilityChanged: updateInputReady()
    onWidthChanged: updateInputReady()
    onHeightChanged: updateInputReady()
    onClosing: function(close) { close.accepted = false; finish() }

    Timer { id: previewTimer; interval: 1800; onTriggered: picker.finish() }

    Rectangle { anchors.fill: parent; color: picker.previewOnly ? "transparent" : "#18151a20" }

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        cursorShape: picker.inputReady ? Qt.CrossCursor : Qt.BusyCursor
        onPositionChanged: function(mouse) {
            if (picker.adjustable && !picker.positionPinned) picker.setTarget(mouse.x, mouse.y)
        }
        onPressed: function(mouse) {
            if (mouse.button === Qt.LeftButton) {
                if (!picker.active) picker.requestActivate()
                keyboard.forceActiveFocus()
                picker.focusSelection()
            }
        }
        onClicked: function(mouse) {
            if (mouse.button === Qt.RightButton || picker.previewOnly) picker.finish()
            else if (picker.inputReady) {
                picker.setTarget(mouse.x, mouse.y)
                picker.positionPinned = true
            }
        }
    }

    Item {
        x: picker.targetX
        y: picker.targetY
        visible: picker.inputReady
        Rectangle { x: -18; y: -1; width: 37; height: 3; color: "black" }
        Rectangle { x: -1; y: -18; width: 3; height: 37; color: "black" }
        Rectangle { x: -17; width: 35; height: 1; color: "white" }
        Rectangle { y: -17; width: 1; height: 35; color: "white" }
        Rectangle {
            x: -9; y: -9; width: 19; height: 19
            radius: 10; color: "transparent"; border.width: 2; border.color: "#3daee9"
        }
    }

    Rectangle {
        anchors.horizontalCenter: parent.horizontalCenter
        y: picker.hintAtBottom ? picker.height - height - 12 : 12
        width: Math.min(picker.width - 24, message.implicitWidth + 32)
        height: message.implicitHeight + 20
        radius: Kirigami.Units.cornerRadius
        color: "#ee151a20"
        border.color: "#3daee9"
        // No input handler: every point, including the hint, is selectable.
        Controls.Label {
            id: message
            anchors.centerIn: parent
            width: Math.min(implicitWidth, picker.width - 56)
            color: "white"
            wrapMode: Text.WordWrap
            horizontalAlignment: Text.AlignHCenter
            text: picker.previewOnly
                ? qsTr("Saved position · X: %1 · Y: %2").arg(picker.targetX).arg(picker.targetY)
                : !picker.inputReady ? qsTr("Opening position selection… · Esc: cancel")
                : qsTr("%1 · X: %2 · Y: %3\nLeft click: choose · Enter: confirm\nArrow keys: move · Shift + arrow keys: 10 steps · Esc / right click: cancel")
                    .arg(picker.screenName).arg(picker.targetX).arg(picker.targetY)
        }
    }

    Shortcut { enabled: picker.visible; sequences: [StandardKey.Cancel]; onActivated: picker.finish() }
    // Window shortcuts work even when Wayland changes the focused Quick item.
    // Keep them scoped to this window so they cannot affect the host controls.
    Item { id: keyboard; focus: true }
    Shortcut { enabled: picker.adjustable; sequences: ["Return", "Enter"]; context: Qt.WindowShortcut; onActivated: picker.confirm() }
    Shortcut { enabled: picker.adjustable; sequence: "Left"; context: Qt.WindowShortcut; onActivated: picker.moveTarget(-1, 0) }
    Shortcut { enabled: picker.adjustable; sequence: "Right"; context: Qt.WindowShortcut; onActivated: picker.moveTarget(1, 0) }
    Shortcut { enabled: picker.adjustable; sequence: "Up"; context: Qt.WindowShortcut; onActivated: picker.moveTarget(0, -1) }
    Shortcut { enabled: picker.adjustable; sequence: "Down"; context: Qt.WindowShortcut; onActivated: picker.moveTarget(0, 1) }
    Shortcut { enabled: picker.adjustable; sequence: "Shift+Left"; context: Qt.WindowShortcut; onActivated: picker.moveTarget(-10, 0) }
    Shortcut { enabled: picker.adjustable; sequence: "Shift+Right"; context: Qt.WindowShortcut; onActivated: picker.moveTarget(10, 0) }
    Shortcut { enabled: picker.adjustable; sequence: "Shift+Up"; context: Qt.WindowShortcut; onActivated: picker.moveTarget(0, -10) }
    Shortcut { enabled: picker.adjustable; sequence: "Shift+Down"; context: Qt.WindowShortcut; onActivated: picker.moveTarget(0, 10) }
}
