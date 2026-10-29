import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import io.github.jerry0205.klickmeister

Kirigami.ApplicationWindow {
    id: root

    property bool smokeTest: false
    property bool monitorSelectionReady: false
    property var selectedMonitor: null
    property var positionPicker: null
    property bool monitorRestored: false
    // Keeps the stop reason visible until the stopped run has fully ended.
    property bool monitorStopPending: false
    property bool monitorSelectionInProgress: false
    property bool positionWasClamped: false
    property bool discardSettingsOnClose: false
    readonly property bool fixedPosition: !controller.current_position
    readonly property bool controlsEnabled: !controller.running && !controller.busy
    readonly property var monitorOptions: {
        const screens = Qt.application.screens
        const options = []
        for (let i = 0; i < screens.length; ++i) {
            options.push({ screen: screens[i], label: monitors.displayName(screens[i], screens) })
        }
        return options
    }
    readonly property string rateDescription: describeRate(intervalInput.value, controller.click_type)

    // SpinBox parses its text on focus loss or Enter and clamps the result to
    // from/to. Empty or unfinished text keeps the last valid value instead of
    // silently becoming a limit such as the 10 ms maximum rate or a screen edge.
    function parseSpinBoxText(spinBox, text, locale) {
        let parsed = NaN
        try { parsed = Number.fromLocaleString(locale, text) } catch (error) {}
        return Number.isInteger(parsed) && parsed >= spinBox.from && parsed <= spinBox.to
            ? parsed : spinBox.value
    }

    function formatRateNumber(value, decimalPlaces) {
        return value.toLocaleString(Qt.locale(), 'f', decimalPlaces)
            .replace(/([,.]\d*?)0+$/, "$1")
            .replace(/[,.]$/, "")
    }

    function describeRate(intervalMs, clickType) {
        const clicksPerCycle = clickType === 1 ? 2 : 1
        if (intervalMs <= 1000) {
            const cyclesPerSecond = 1000 / intervalMs
            // Three places distinguish every whole-millisecond interval below
            // one second from an exact rate of one cycle per second.
            const cycleRate = Number(cyclesPerSecond.toFixed(3))
            const clickRate = Number((cyclesPerSecond * clicksPerCycle).toFixed(3))
            const cycleUnit = cycleRate === 1 ? qsTr("Zyklus/s") : qsTr("Zyklen/s")
            const clickUnit = clickRate === 1 ? qsTr("Klick/s") : qsTr("Klicks/s")
            return qsTr("%1 %2 · %3 %4")
                .arg(formatRateNumber(cycleRate, 3)).arg(cycleUnit)
                .arg(formatRateNumber(clickRate, 3)).arg(clickUnit)
        }
        const seconds = formatRateNumber(intervalMs / 1000, 3)
        const clicks = clicksPerCycle === 1 ? qsTr("1 Klick") : qsTr("2 Klicks")
        return qsTr("1 Zyklus alle %1 s · %2 alle %1 s").arg(seconds).arg(clicks)
    }

    function finishPicker(restoreHost) {
        if (positionPicker) positionPicker.finish(restoreHost)
    }

    // A later launch asks this instance to show itself. During a position
    // selection, return to the picker instead of covering it.
    function activateFromLauncher() {
        const target = positionPicker && positionPicker.visible ? positionPicker : root
        target.raise()
        target.requestActivate()
    }

    function beginPicker(preview) {
        if (positionPicker || !selectedMonitor) return
        const picker = positionPickerComponent.createObject(root, { "screen": selectedMonitor })
        if (!picker) return
        positionPicker = picker
        picker.begin(selectedMonitor, controller.fixed_x, controller.fixed_y, preview)
    }

    function confirmMonitor() {
        syncMonitor()
        controller.fixed_position_confirmed = !!selectedMonitor
        markPositionChanged()
    }

    function markPositionChanged() {
        positionWasClamped = false
        controller.mark_position_changed()
    }

    // Coordinates clamped to a smaller monitor were never chosen by the user.
    function selectMonitor(screen) {
        const sameSelection = screen === selectedMonitor
        const wasConfirmed = controller.fixed_position_confirmed
        monitorSelectionInProgress = true
        selectedMonitor = screen
        monitorSelectionInProgress = false
        const fits = syncMonitor()
        // Switching screens must not accept a point clamped by an earlier
        // selection or geometry change, even if it now fits the new screen.
        const confirmed = fits && !positionWasClamped && (!sameSelection || wasConfirmed)
        controller.fixed_position_confirmed = confirmed
        // Clamping to a smaller monitor is not a chosen position. Keep the
        // saved coordinates until the user confirms or edits the displayed ones.
        if (confirmed && !sameSelection) markPositionChanged()
    }

    function syncMonitor() {
        const screen = selectedMonitor
        const identity = screen ? monitors.identity(screen) : ""
        const x = screen ? screen.virtualX : 0
        const y = screen ? screen.virtualY : 0
        const width = screen ? screen.width : 0
        const height = screen ? screen.height : 0
        const moved = identity !== controller.monitor_identity
            || x !== controller.monitor_x || y !== controller.monitor_y
            || width !== controller.monitor_width || height !== controller.monitor_height
        // Cursor-position runs never use the selected monitor.
        if ((controller.running || controller.busy) && !controller.current_position
                && (moved || !controller.fixed_position_confirmed)) {
            monitorStopPending = true
            monitorStopMessage.visible = true
            controller.stop()
        }
        controller.monitor_identity = identity
        controller.monitor_x = x
        controller.monitor_y = y
        controller.monitor_width = width
        controller.monitor_height = height
        // Reselecting the saved monitor shows its saved position before clamping.
        // Those are the user's own coordinates, not a point clamped earlier.
        if (controller.restore_saved_position()) positionWasClamped = false
        const fits = !!screen && controller.fixed_x < width && controller.fixed_y < height
        const clampedX = Math.max(0, Math.min(controller.fixed_x, xInput.to))
        const clampedY = Math.max(0, Math.min(controller.fixed_y, yInput.to))
        if (clampedX !== controller.fixed_x || clampedY !== controller.fixed_y)
            positionWasClamped = true
        controller.fixed_x = clampedX
        controller.fixed_y = clampedY
        // SpinBox initially clamps saved coordinates to its zero-sized monitor.
        // Restore the display even when the controller's coordinate is unchanged.
        xInput.value = controller.fixed_x
        yInput.value = controller.fixed_y
        return fits
    }

    onSelectedMonitorChanged: if (!monitorSelectionInProgress) syncMonitor()

    function ensureMonitorSelection() {
        if (!monitorSelectionReady) {
            return
        }

        const screens = Qt.application.screens
        if (!monitorRestored) {
            monitorRestored = true
            const restored = monitors.restoreIndex(screens, controller.monitor_identity)
            if (restored >= 0) {
                const screen = screens[restored]
                const valid = controller.fixed_x < screen.width && controller.fixed_y < screen.height
                selectedMonitor = screen
                monitorInput.currentIndex = restored
                controller.fixed_position_confirmed = valid
                return
            }
        }
        for (let index = 0; index < screens.length; ++index) {
            if (screens[index] === selectedMonitor) {
                monitorInput.currentIndex = index
                return
            }
        }

        controller.fixed_position_confirmed = false
        for (let index = 0; index < screens.length; ++index) {
            if (screens[index] === root.screen) {
                selectedMonitor = screens[index]
                monitorInput.currentIndex = index
                return
            }
        }

        selectedMonitor = screens.length > 0 ? screens[0] : null
        monitorInput.currentIndex = screens.length > 0 ? 0 : -1
    }

    width: Kirigami.Units.gridUnit * 25
    height: Kirigami.Units.gridUnit * 29
    minimumWidth: Kirigami.Units.gridUnit * 20
    minimumHeight: Kirigami.Units.gridUnit * 16
    visible: true
    title: qsTr("Klickmeister %1").arg(Qt.application.version)

    MonitorSelection { id: monitors }

    AppController {
        id: controller
        objectName: "controller"
    }

    Component {
        id: positionPickerComponent
        PositionPicker {
            id: picker
            hostWindow: root
            onSelectingChanged: controller.selecting_position = selecting
            onPicked: function(x, y) {
                // KWin can place fullscreen windows on another output. Resolve
                // the actual screen before saving its local coordinates. The
                // picker's Screen wrapper is destroyed with its window, so keep
                // the corresponding entry from the application's screen list.
                const screens = Qt.application.screens
                const index = picker.screen
                    ? monitors.restoreIndex(screens, monitors.identity(picker.screen)) : -1
                const positionError = qsTr("Der gewählte Punkt kann keinem Bildschirm zugeordnet werden. Wähle die Position erneut.")
                if (index < 0 || x < 0 || y < 0
                        || x >= screens[index].width || y >= screens[index].height) {
                    controller.fixed_position_confirmed = false
                    controller.error_message = positionError
                    return
                }
                if (controller.error_message === positionError) controller.clear_error()
                root.selectMonitor(screens[index])
                monitorInput.currentIndex = index
                controller.fixed_x = x
                controller.fixed_y = y
                controller.current_position = false
                controller.fixed_position_confirmed = true
                root.markPositionChanged()
            }
            onFinished: {
                if (root.positionPicker === picker) root.positionPicker = null
                picker.destroy()
            }
        }
    }

    Connections {
        target: controller

        function onFixed_xChanged() { xInput.value = controller.fixed_x }
        function onFixed_yChanged() { yInput.value = controller.fixed_y }
    }

    Component.onCompleted: {
        ensureMonitorSelection()
        if (!smokeTest) {
            controller.initialize()
        }
    }
    onScreenChanged: ensureMonitorSelection()
    // A stop is asynchronous, so a run may still report Running(true) first.
    function updateMonitorStopMessage() {
        if (!controller.running && !controller.busy) monitorStopPending = false
        else if (!monitorStopPending) monitorStopMessage.visible = false
    }
    function handleScreensChanged() {
        finishPicker()
        ensureMonitorSelection()
        syncMonitor()
    }
    function handleMonitorGeometryChanged(invalidatePosition) {
        if (invalidatePosition) controller.fixed_position_confirmed = false
        finishPicker()
        syncMonitor()
    }
    Connections {
        target: Qt.application
        function onScreensChanged() { root.handleScreensChanged() }
    }
    Connections {
        target: root.selectedMonitor
        function onWidthChanged() { root.handleMonitorGeometryChanged(true) }
        function onHeightChanged() { root.handleMonitorGeometryChanged(true) }
        function onDevicePixelRatioChanged() { root.handleMonitorGeometryChanged(true) }
        function onVirtualXChanged() { root.handleMonitorGeometryChanged(false) }
        function onVirtualYChanged() { root.handleMonitorGeometryChanged(false) }
    }
    onClosing: function(close) {
        root.finishPicker(false)
        controller.shutdown()
        if (!root.discardSettingsOnClose && !controller.save_config()) {
            close.accepted = false
            if (root.visibility === Window.Minimized || root.visibility === Window.Hidden)
                root.showNormal()
            root.raise()
            root.requestActivate()
            saveFailureDialog.open()
            return
        }
        close.accepted = true
    }

    Controls.Dialog {
        id: saveFailureDialog
        objectName: "saveFailureDialog"
        modal: true
        closePolicy: Controls.Popup.NoAutoClose
        title: qsTr("Einstellungen nicht gespeichert")
        width: Math.min(root.width - 32, 440)
        x: (root.width - width) / 2
        y: (root.height - height) / 2
        // The header's close button rejects the dialog; treat it as continuing.
        onRejected: controller.initialize()

        ColumnLayout {
            width: saveFailureDialog.availableWidth
            Controls.Label {
                Layout.fillWidth: true
                text: controller.error_message
                wrapMode: Text.WordWrap
            }
            Controls.Label {
                Layout.fillWidth: true
                text: qsTr("Beim Fortsetzen fragt KDE erneut nach der Freigabe für das Tastenkürzel. Die Freigabe für die Maussteuerung folgt beim nächsten Start.")
                wrapMode: Text.WordWrap
                color: Kirigami.Theme.disabledTextColor
            }
            ColumnLayout {
                Layout.fillWidth: true
                Controls.Button {
                    objectName: "continueEditingButton"
                    Layout.fillWidth: true
                    text: qsTr("Zurück zur Anwendung")
                    onClicked: {
                        saveFailureDialog.close()
                        controller.initialize()
                    }
                }
                Controls.Button {
                    objectName: "discardSettingsButton"
                    Layout.fillWidth: true
                    text: qsTr("Ohne Speichern schließen")
                    onClicked: {
                        saveFailureDialog.close()
                        root.discardSettingsOnClose = true
                        root.close()
                    }
                }
            }
        }
    }

    footer: Controls.Pane {
        objectName: "runFooter"
        padding: Kirigami.Units.smallSpacing
        background: Rectangle {
            color: Kirigami.Theme.backgroundColor
            Kirigami.Separator { width: parent.width }
        }

        contentItem: ColumnLayout {
            spacing: Kirigami.Units.smallSpacing

            RowLayout {
                Layout.fillWidth: true
                Controls.Label {
                    objectName: "hotkeyStatusLabel"
                    Layout.fillWidth: true
                    text: controller.hotkey_configuring ? qsTr("Dialog öffnet …") :
                          controller.hotkey_pending ? qsTr("Wird eingerichtet …") :
                              (controller.hotkey_ready ? controller.hotkey : qsTr("Nicht verfügbar"))
                    elide: Text.ElideRight
                    Accessible.name: qsTr("Tastenkürzel: %1").arg(text)
                }
                Controls.ToolButton {
                    objectName: "hotkeyConfigureButton"
                    icon.name: "input-keyboard"
                    text: controller.hotkey_ready ? qsTr("Ändern …") : qsTr("Erneut versuchen")
                    enabled: root.controlsEnabled && !controller.hotkey_pending
                    onClicked: controller.configure_hotkey()
                    Controls.ToolTip.text: qsTr("Tastenkürzel für Start und Stopp ändern")
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.delay: Kirigami.Units.toolTipDelay
                }
            }

            Controls.Button {
                readonly property bool stopMode: controller.running || controller.busy
                // A button start at the cursor counts down after the permission.
                readonly property int countdown: stopMode ? controller.countdown_remaining : 0

                objectName: "startStopButton"
                Layout.fillWidth: true
                Layout.preferredHeight: Kirigami.Units.gridUnit * 2.6
                highlighted: true
                text: countdown > 0 ? qsTr("■  Start abbrechen (%1)").arg(countdown)
                    : stopMode ? qsTr("■  Stoppen") : qsTr("▶  Starten")
                enabled: stopMode || (controller.hotkey_ready && !controller.hotkey_pending)
                Accessible.name: countdown > 1 ? qsTr("Start abbrechen, Klicken beginnt in %1 Sekunden").arg(countdown)
                    : countdown === 1 ? qsTr("Start abbrechen, Klicken beginnt in 1 Sekunde")
                    : stopMode ? qsTr("Stoppen") : qsTr("Starten")
                onClicked: stopMode ? controller.stop() : controller.start_from_button()
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Rectangle {
                    implicitWidth: Kirigami.Units.smallSpacing
                    implicitHeight: implicitWidth
                    radius: implicitWidth / 2
                    color: controller.running ? Kirigami.Theme.positiveTextColor
                        : controller.error_message.length > 0 ? Kirigami.Theme.negativeTextColor
                        : Kirigami.Theme.disabledTextColor
                }
                Controls.Label {
                    objectName: "statusLabel"
                    Layout.fillWidth: true
                    text: controller.running
                        ? qsTr("Status: %1 · %2").arg(controller.status).arg(root.rateDescription)
                        : qsTr("Status: %1").arg(controller.status)
                    wrapMode: Text.WordWrap
                }
                Controls.BusyIndicator {
                    visible: controller.busy
                    running: visible
                    implicitWidth: Kirigami.Units.gridUnit
                    implicitHeight: implicitWidth
                }
            }
        }
    }

    pageStack.globalToolBar.style: Kirigami.ApplicationHeaderStyle.None
    pageStack.initialPage: Kirigami.ScrollablePage {
        id: settingsPage
        header: Controls.ScrollView {
            id: messageHeader
            objectName: "messageHeader"
            // Long portal errors must leave room for the settings and run controls.
            implicitHeight: Math.min(messageColumn.implicitHeight, root.height / 3)
            contentWidth: availableWidth
            Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff

            ColumnLayout {
                id: messageColumn
                width: messageHeader.availableWidth
                Kirigami.InlineMessage {
                    id: errorBanner
                    objectName: "errorBanner"
                    Layout.fillWidth: true
                    visible: false
                    type: Kirigami.MessageType.Error
                    text: controller.error_message
                    showCloseButton: true
                    onVisibleChanged: if (!visible) controller.clear_error()
                    Component.onCompleted: visible = controller.error_message.length > 0
                }
                Kirigami.InlineMessage {
                    id: monitorStopMessage
                    objectName: "monitorStopMessage"
                    Layout.fillWidth: true
                    visible: false
                    type: Kirigami.MessageType.Warning
                    text: qsTr("Klicken gestoppt: Der Bildschirm der festen Position hat sich geändert. Prüfe die Position vor dem nächsten Start.")
                    showCloseButton: true
                }
            }
        }

        Connections {
            target: controller
            function onError_messageChanged() {
                errorBanner.visible = controller.error_message.length > 0
            }
            function onRunningChanged() { root.updateMonitorStopMessage() }
            function onBusyChanged() { root.updateMonitorStopMessage() }
        }

        Connections {
            target: root
            function onActiveFocusItemChanged() {
                const item = root.activeFocusItem
                let ancestor = item
                while (ancestor && ancestor !== settingsForm) ancestor = ancestor.parent
                if (!ancestor) return
                const position = item.mapToItem(settingsPage.flickable.contentItem, 0, 0)
                settingsPage.ensureVisible(item, position.x - item.x, position.y - item.y)
            }
        }

        GridLayout {
            id: settingsForm
            columns: width >= Kirigami.Units.gridUnit * 22 ? 2 : 1
            columnSpacing: Kirigami.Units.largeSpacing
            rowSpacing: Kirigami.Units.smallSpacing
            enabled: root.controlsEnabled
            Accessible.role: Accessible.Form

            Controls.Label {
                text: qsTr("Maustaste:")
                Layout.alignment: settingsForm.columns === 2 ? Qt.AlignRight : Qt.AlignLeft
            }
            ChoiceButtons {
                objectName: "mouseButtonInput"
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                iconOnly: true
                value: controller.mouse_button
                options: [
                    { value: 0, icon: "input-mouse-click-left", text: qsTr("Links") },
                    { value: 2, icon: "input-mouse-click-middle", text: qsTr("Mitte") },
                    { value: 1, icon: "input-mouse-click-right", text: qsTr("Rechts") }
                ]
                onSelected: choice => { controller.mouse_button = choice; controller.mark_settings_changed() }
            }

            Controls.Label {
                text: qsTr("Klick:")
                Layout.alignment: settingsForm.columns === 2 ? Qt.AlignRight : Qt.AlignLeft
            }
            ChoiceButtons {
                objectName: "clickTypeInput"
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                value: controller.click_type
                options: [
                    { value: 0, text: qsTr("1×"), description: qsTr("Einzelklick") },
                    { value: 1, text: qsTr("2×"), description: qsTr("Doppelklick") }
                ]
                onSelected: choice => { controller.click_type = choice; controller.mark_settings_changed() }
            }

            Controls.Label {
                text: qsTr("Intervall:")
                Layout.alignment: settingsForm.columns === 2 ? Qt.AlignRight : Qt.AlignLeft
            }
            RowLayout {
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                Controls.SpinBox {
                    id: intervalInput
                    objectName: "intervalInput"
                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    from: 10
                    to: 86400000
                    stepSize: 10
                    editable: true
                    valueFromText: (text, locale) => root.parseSpinBoxText(intervalInput, text, locale)
                    wheelEnabled: false
                    value: controller.interval_ms
                    onValueModified: { controller.interval_ms = value; controller.mark_settings_changed() }
                    Accessible.name: qsTr("Klickintervall in Millisekunden")
                }
                Controls.Label { text: qsTr("ms") }
            }

            Controls.Label {
                objectName: "rateLabel"
                Layout.columnSpan: settingsForm.columns
                Layout.fillWidth: true
                color: Kirigami.Theme.disabledTextColor
                text: root.rateDescription
                wrapMode: Text.WordWrap
            }

            Controls.Label {
                text: qsTr("Wiederholen:")
                Layout.alignment: settingsForm.columns === 2 ? Qt.AlignRight : Qt.AlignLeft
            }
            ChoiceButtons {
                objectName: "repeatModeInput"
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                value: controller.repeat_until_stopped
                options: [
                    { value: true, text: "∞", description: qsTr("Bis zum Stoppen") },
                    { value: false, text: qsTr("Klickzyklen") }
                ]
                onSelected: choice => { controller.repeat_until_stopped = choice; controller.mark_settings_changed() }
            }
            Controls.Label {
                text: qsTr("Anzahl:")
                visible: !controller.repeat_until_stopped
                Layout.alignment: settingsForm.columns === 2 ? Qt.AlignRight : Qt.AlignLeft
            }
            Controls.SpinBox {
                id: repeatInput
                objectName: "repeatInput"
                visible: !controller.repeat_until_stopped
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                from: 1
                to: 10000000
                editable: true
                valueFromText: (text, locale) => root.parseSpinBoxText(repeatInput, text, locale)
                wheelEnabled: false
                value: controller.repeat_count
                onValueModified: { controller.repeat_count = value; controller.mark_settings_changed() }
                Accessible.name: qsTr("Anzahl der Klickzyklen")
            }

            Kirigami.Separator {
                Layout.columnSpan: settingsForm.columns
                Layout.fillWidth: true
            }

            Controls.Label {
                text: qsTr("Ziel:")
                Layout.alignment: settingsForm.columns === 2 ? Qt.AlignRight : Qt.AlignLeft
            }
            ChoiceButtons {
                objectName: "positionModeInput"
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                value: controller.current_position
                options: [
                    { value: true, icon: "cursor-arrow", text: qsTr("Mauszeiger"),
                      description: qsTr("Aktuelle Mauszeigerposition") },
                    { value: false, icon: "crosshairs", text: qsTr("Fester Punkt"),
                      description: qsTr("Feste Position") }
                ]
                onSelected: choice => { controller.current_position = choice; controller.mark_settings_changed() }
            }

            Controls.Label {
                text: qsTr("Bildschirm:")
                visible: root.fixedPosition
                Layout.alignment: settingsForm.columns === 2 ? Qt.AlignRight : Qt.AlignLeft
            }
            Controls.ComboBox {
                id: monitorInput
                objectName: "monitorInput"
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                visible: root.fixedPosition
                model: root.monitorOptions
                textRole: "label"
                onModelChanged: Qt.callLater(root.ensureMonitorSelection)
                Component.onCompleted: {
                    root.monitorSelectionReady = true
                    root.ensureMonitorSelection()
                }
                wheelEnabled: false
                onActivated: root.selectMonitor(model[currentIndex].screen)
                Accessible.name: qsTr("Bildschirm für die feste Position")
            }

            Controls.Label {
                text: qsTr("Punkt:")
                visible: root.fixedPosition
                Layout.alignment: settingsForm.columns === 2 ? Qt.AlignRight : Qt.AlignLeft
            }
            RowLayout {
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                visible: root.fixedPosition
                Controls.Label { text: "X" }
                Controls.SpinBox {
                    id: xInput
                    objectName: "xInput"
                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    from: 0
                    to: Math.max(0, controller.monitor_width - 1)
                    editable: true
                    valueFromText: (text, locale) => root.parseSpinBoxText(xInput, text, locale)
                    wheelEnabled: false
                    value: controller.fixed_x
                    onValueModified: { controller.fixed_x = value; root.markPositionChanged() }
                    Accessible.name: qsTr("X-Koordinate auf dem gewählten Bildschirm")
                }
                Controls.Label { text: "Y" }
                Controls.SpinBox {
                    id: yInput
                    objectName: "yInput"
                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    from: 0
                    to: Math.max(0, controller.monitor_height - 1)
                    editable: true
                    valueFromText: (text, locale) => root.parseSpinBoxText(yInput, text, locale)
                    wheelEnabled: false
                    value: controller.fixed_y
                    onValueModified: { controller.fixed_y = value; root.markPositionChanged() }
                    Accessible.name: qsTr("Y-Koordinate auf dem gewählten Bildschirm")
                }
            }

            RowLayout {
                Layout.columnSpan: settingsForm.columns
                visible: root.fixedPosition
                Controls.Button {
                    objectName: "pickButton"
                    icon.name: "crosshairs"
                    text: qsTr("Auswählen …")
                    enabled: !!root.selectedMonitor
                    onClicked: root.beginPicker(false)
                    Controls.ToolTip.text: qsTr("Punkt im Vollbild auf dem Bildschirm wählen")
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.delay: Kirigami.Units.toolTipDelay
                }
                Controls.ToolButton {
                    objectName: "previewButton"
                    icon.name: "view-visible"
                    text: qsTr("Punkt anzeigen")
                    display: Controls.AbstractButton.IconOnly
                    enabled: !!root.selectedMonitor && controller.fixed_position_confirmed
                    onClicked: root.beginPicker(true)
                    Controls.ToolTip.text: text
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.delay: Kirigami.Units.toolTipDelay
                }
            }

            Kirigami.InlineMessage {
                objectName: "confirmMessage"
                Layout.columnSpan: settingsForm.columns
                Layout.fillWidth: true
                visible: root.fixedPosition && !controller.fixed_position_confirmed
                type: Kirigami.MessageType.Warning
                text: qsTr("Prüfe Bildschirm und Koordinaten und bestätige die Position oder wähle sie neu.")
            }

            Controls.Button {
                objectName: "confirmMonitorButton"
                Layout.columnSpan: settingsForm.columns
                visible: root.fixedPosition && !controller.fixed_position_confirmed
                enabled: !!root.selectedMonitor
                icon.name: "dialog-ok-apply"
                text: qsTr("Position bestätigen")
                onClicked: root.confirmMonitor()
            }

            Controls.Label {
                Layout.columnSpan: settingsForm.columns
                Layout.fillWidth: true
                visible: root.fixedPosition
                wrapMode: Text.WordWrap
                font: Kirigami.Theme.smallFont
                color: Kirigami.Theme.disabledTextColor
                text: qsTr("Das Hauptfenster kann auf einem anderen Bildschirm bleiben.")
            }
        }
    }
}
