import QtQuick
import QtTest
import QtQuick.Controls as Controls
import "../../qml"

TestCase {
    name: "MainControls"
    when: windowShown
    width: 600
    height: 800

    Component { id: mainComponent; Main { smokeTest: true } }
    Component {
        id: monitorComponent
        QtObject {
            property string name: ""
            property string manufacturer: ""
            property string model: ""
            property string serialNumber: ""
            property int width: 3840
            property int height: 2160
            property int virtualX: 0
            property int virtualY: 0
            property real devicePixelRatio: 1
        }
    }
    property var main
    property var controller

    function init() {
        main = createTemporaryObject(mainComponent, null)
        verify(main !== null)
        controller = findChild(main, "controller")
        verify(controller !== null)
        main.requestActivate()
        tryCompare(main, "active", true)
    }
    function cleanup() { main.close() }

    function findItem(item, predicate) {
        if (predicate(item)) return item
        for (const child of item.children) {
            const found = findItem(child, predicate)
            if (found) return found
        }
        return null
    }

    function choice(groupName, label) {
        const group = findChild(main, groupName)
        verify(group !== null)
        const button = findItem(group, item => item.Accessible.name === label && item.checkable)
        verify(button !== null, label)
        return button
    }

    function test_choice_buttons_update_controller_data() {
        return [
            { tag: "right button", group: "mouseButtonInput", label: "Rechts", property: "mouse_button", value: 1 },
            { tag: "middle button", group: "mouseButtonInput", label: "Mitte", property: "mouse_button", value: 2 },
            { tag: "double click", group: "clickTypeInput", label: "Doppelklick", property: "click_type", value: 1 },
            { tag: "repeat count", group: "repeatModeInput", label: "Klickzyklen", property: "repeat_until_stopped", value: false },
            { tag: "fixed position", group: "positionModeInput", label: "Feste Position", property: "current_position", value: false }
        ]
    }
    function test_choice_buttons_update_controller(data) {
        const button = choice(data.group, data.label)
        verify(!button.checked)
        mouseClick(button)
        compare(controller[data.property], data.value)
        verify(button.checked)
        mouseClick(button)
        verify(button.checked, "clicking the selected option keeps it selected")
        compare(button.Accessible.role, Accessible.RadioButton)
    }

    function test_fixed_position_controls_only_show_for_fixed_position() {
        for (const fixed of [false, true]) {
            controller.current_position = !fixed
            for (const name of ["monitorInput", "xInput", "yInput", "pickButton", "previewButton"])
                compare(findChild(main, name).visible, fixed, name)
        }
    }

    function test_settings_fit_window_after_position_mode_changes_data() {
        return [
            { tag: "minimum", width: 360 },
            { tag: "default", width: 450 },
            { tag: "wide", width: 650 }
        ]
    }
    function test_settings_fit_window_after_position_mode_changes(data) {
        main.width = data.width
        controller.interval_ms = 86400000
        controller.repeat_until_stopped = false
        controller.repeat_count = 10000000
        for (const fixed of [true, false, true]) {
            controller.current_position = !fixed
            wait(50)
            for (const name of ["intervalInput", "repeatInput", "monitorInput", "xInput", "yInput", "rateLabel", "positionModeInput"]) {
                const field = findChild(main, name)
                if (!field.visible) continue
                const left = field.mapToItem(null, 0, 0).x
                verify(left >= 0 && left + field.width <= main.width,
                       name + " spans " + left + " to " + (left + field.width) + " in " + main.width)
            }
        }
    }

    function test_keyboard_focus_scrolls_settings_into_view() {
        main.width = main.minimumWidth
        main.height = main.minimumHeight
        controller.current_position = false
        const input = findChild(main, "yInput")
        waitForRendering(input)
        input.forceActiveFocus(Qt.TabFocusReason)
        const flickable = main.pageStack.currentItem.flickable
        tryVerify(() => {
            const point = input.mapToItem(flickable, 0, 0)
            return point.y >= 0 && point.y + input.height <= flickable.height
        })
    }

    function test_mouse_icon_buttons_have_tooltips() {
        const button = choice("mouseButtonInput", "Links")
        // The offscreen platform disables hover effects by default.
        button.hoverEnabled = true
        waitForRendering(button)
        mouseMove(button, button.width / 2, button.height / 2)
        tryCompare(button, "hovered", true)
        tryCompare(button.Controls.ToolTip, "visible", true)
        compare(button.Controls.ToolTip.text, "Links")
    }

    function test_saved_coordinates_are_visible_after_monitor_initialization() {
        compare(controller.fixed_x, 80)
        compare(controller.fixed_y, 150)
        compare(findChild(main, "xInput").value, controller.fixed_x)
        compare(findChild(main, "yInput").value, controller.fixed_y)
    }

    function closeButtonIn(item) {
        if (item.icon && item.icon.name === "dialog-close") return item
        for (const child of item.children) {
            const button = closeButtonIn(child)
            if (button) return button
        }
        return null
    }

    function test_error_banner_reappears_after_close() {
        const banner = findChild(main, "errorBanner")
        verify(banner !== null)

        for (const message of ["First error", "Second error", "Second error"]) {
            controller.error_message = message
            tryCompare(banner, "visible", true)
            compare(banner.text, message)

            const closeButton = closeButtonIn(banner)
            verify(closeButton !== null)
            // The header animates open; wait until its close button is on screen.
            tryVerify(() => closeButton.mapToItem(null, 0, 0).y >= 0)
            waitForRendering(closeButton)
            mouseClick(closeButton)
            tryCompare(banner, "visible", false)
            compare(controller.error_message, "")
        }
    }

    function test_typed_values_reach_hotkey_without_focus_loss_data() {
        return [
            { tag: "interval", input: "intervalInput", property: "interval_ms", value: 250, position: false },
            { tag: "repeat", input: "repeatInput", property: "repeat_count", value: 42, position: false },
            { tag: "x", input: "xInput", property: "fixed_x", value: 123, position: true },
            { tag: "y", input: "yInput", property: "fixed_y", value: 234, position: true }
        ]
    }
    function test_typed_values_reach_hotkey_without_focus_loss(data) {
        controller.repeat_until_stopped = false
        controller.current_position = false
        const input = findChild(main, data.input)
        verify(input !== null)
        input.contentItem.forceActiveFocus()
        keyClick(Qt.Key_A, Qt.ControlModifier)
        for (const digit of String(data.value)) keyClick(digit)
        verify(input.contentItem.activeFocus)
        compare(input.contentItem.text, String(data.value))
        compare(input.value, data.value)
        compare(controller[data.property], data.value)
        compare(controller.configDirty, true)
        compare(controller.positionDirty, data.position)
    }

    function replaceText(input, text) {
        input.contentItem.forceActiveFocus()
        keyClick(Qt.Key_A, Qt.ControlModifier)
        keyClick(Qt.Key_Backspace)
        for (const digit of text) keyClick(digit)
        compare(input.contentItem.text, text)
    }

    // SpinBox parses unfinished text on focus loss and clamps it to from/to,
    // which turned an empty interval into the 10 ms maximum rate.
    function test_start_button_keeps_interval_of_unfinished_input_data() {
        return [
            { tag: "empty", text: "" },
            { tag: "below minimum", text: "5" }
        ]
    }
    function test_start_button_keeps_interval_of_unfinished_input(data) {
        controller.interval_ms = 1000
        const input = findChild(main, "intervalInput")
        const start = findChild(main, "startStopButton")
        let startedWith = -1
        start.clicked.connect(() => { startedWith = controller.interval_ms })
        replaceText(input, data.text)
        mouseClick(start)
        compare(controller.button_starts, 1)
        compare(startedWith, 1000)
        compare(input.contentItem.text, "1000")
        compare(controller.configDirty, false)
    }

    function test_cleared_inputs_keep_values_data() {
        const rows = []
        for (const field of [
            { name: "interval", input: "intervalInput", property: "interval_ms" },
            { name: "repeat", input: "repeatInput", property: "repeat_count" },
            { name: "x", input: "xInput", property: "fixed_x" },
            { name: "y", input: "yInput", property: "fixed_y" }
        ]) {
            for (const key of [{ name: "tab", code: Qt.Key_Tab }, { name: "enter", code: Qt.Key_Return }])
                rows.push({ tag: field.name + " " + key.name, input: field.input, property: field.property, key: key.code })
        }
        return rows
    }
    function test_cleared_inputs_keep_values(data) {
        controller.repeat_until_stopped = false
        controller.current_position = false
        const input = findChild(main, data.input)
        const before = controller[data.property]
        replaceText(input, "")
        keyClick(data.key)
        compare(controller[data.property], before)
        compare(input.contentItem.text, String(before))
        compare(controller.configDirty, false)
        compare(controller.positionDirty, false)
    }

    function test_hotkey_start_keeps_interval_of_unfinished_input() {
        controller.interval_ms = 1000
        const input = findChild(main, "intervalInput")
        const rateLabel = findChild(main, "rateLabel")
        replaceText(input, "5")
        const rate = rateLabel.text
        // A hotkey start runs with the last committed value and then
        // disables the form, which takes focus from the unfinished input.
        controller.busy = true
        verify(!input.contentItem.activeFocus)
        compare(controller.interval_ms, 1000)
        compare(input.contentItem.text, "1000")
        compare(rateLabel.text, rate)
        compare(controller.configDirty, false)
    }

    function test_interval_edits_reach_controller_without_group_separators() {
        // A German "1.500" became "1.5000" after typing a digit, which the
        // validator rejects until focus loss. Run with LANG=de_DE.UTF-8 too.
        controller.interval_ms = 1500
        const input = findChild(main, "intervalInput")
        compare(input.contentItem.text, "1500")
        input.contentItem.forceActiveFocus()
        keyClick(Qt.Key_End)
        keyClick("0")
        compare(controller.interval_ms, 15000)
        keyClick(Qt.Key_Backspace)
        keyClick(Qt.Key_Backspace)
        compare(controller.interval_ms, 150)
    }

    function test_monitor_initialization_does_not_replace_saved_position() {
        // Restoring the monitor at startup clamps and rewrites the displayed
        // position, but only user actions may replace the saved one.
        compare(controller.fixed_position_confirmed, false)
        compare(controller.configDirty, false)
        compare(controller.positionDirty, false)
    }

    function test_confirming_monitor_replaces_saved_position() {
        controller.current_position = false
        const confirm = findChild(main, "confirmMonitorButton")
        verify(confirm !== null)
        tryCompare(confirm, "visible", true)
        compare(confirm.enabled, true)
        // The button can lie below the scrollable page's visible area.
        confirm.clicked()
        compare(controller.fixed_position_confirmed, true)
        compare(controller.positionDirty, true)
    }

    function test_reselecting_saved_monitor_restores_saved_position() {
        controller.current_position = false
        // Simulate coordinates clamped to a substitute for the saved monitor.
        controller.savedMonitorIdentity = controller.monitor_identity
        controller.savedFixedX = 300
        controller.savedFixedY = 200
        controller.fixed_x = 10
        controller.fixed_y = 20
        const monitorInput = findChild(main, "monitorInput")
        monitorInput.activated(monitorInput.currentIndex)
        compare(controller.fixed_x, 300)
        compare(controller.fixed_y, 200)
        compare(findChild(main, "xInput").value, 300)
        compare(controller.fixed_position_confirmed, false)
        compare(controller.positionDirty, false)
        main.confirmMonitor()
        compare(controller.fixed_position_confirmed, true)
        compare(controller.positionDirty, true)

        // Restored coordinates are still clamped to the selected monitor.
        controller.fixed_position_confirmed = false
        controller.savedFixedX = 100000
        monitorInput.activated(monitorInput.currentIndex)
        compare(controller.fixed_x, findChild(main, "xInput").to)
        compare(controller.fixed_y, 200)
    }

    function test_explicit_monitor_selection_replaces_missing_saved_monitor() {
        controller.current_position = false
        controller.savedMonitorIdentity = "missing-monitor"
        controller.savedFixedX = 2400
        controller.savedFixedY = 1300
        controller.configDirty = false
        controller.positionDirty = false

        const screen = main.monitorOptions[0].screen
        main.selectMonitor(screen)

        compare(controller.fixed_position_confirmed, false)
        compare(controller.positionDirty, false)
        main.confirmMonitor()
        compare(controller.fixed_position_confirmed, true)
        compare(controller.configDirty, true)
        compare(controller.positionDirty, true)
        compare(controller.savedMonitorIdentity, controller.monitor_identity)
        compare(controller.savedFixedX, controller.fixed_x)
        compare(controller.savedFixedY, controller.fixed_y)
    }

    function test_selecting_smaller_monitor_requires_confirmation_after_clamping() {
        const smaller = Qt.createQmlObject('import QtQuick; QtObject {'
            + 'property string name: "smaller"; property string manufacturer: "";'
            + 'property string model: ""; property string serialNumber: "";'
            + 'property int width: 100; property int height: 100;'
            + 'property int virtualX: 0; property int virtualY: 0;'
            + 'property real devicePixelRatio: 1}', main)
        verify(smaller !== null)
        controller.current_position = false
        controller.fixed_x = 300
        controller.fixed_y = 200
        controller.fixed_position_confirmed = true

        main.selectMonitor(smaller)

        compare(controller.fixed_x, 99)
        compare(controller.fixed_y, 99)
        compare(controller.fixed_position_confirmed, false)
        compare(controller.positionDirty, false)
        compare(controller.savedMonitorIdentity, "")
    }

    function test_switching_monitors_keeps_clamped_position_unconfirmed_data() {
        return [
            { tag: "confirm", action: "confirm" },
            { tag: "edit x", action: "xInput" },
            { tag: "edit y", action: "yInput" },
            { tag: "pick", action: "pick" },
            { tag: "restore saved monitor", action: "restore" },
            { tag: "geometry change", action: "restore", resize: true }
        ]
    }
    function test_switching_monitors_keeps_clamped_position_unconfirmed(data) {
        const original = createTemporaryObject(monitorComponent, main, { name: "A" })
        const smaller = createTemporaryObject(monitorComponent, main,
            { name: "B", width: 1920, height: 1080 })
        const larger = createTemporaryObject(monitorComponent, main, { name: "C" })
        controller.current_position = false
        main.selectMonitor(original)
        controller.fixed_x = 3000
        controller.fixed_y = 1500
        main.confirmMonitor()
        const savedIdentity = controller.savedMonitorIdentity
        controller.configDirty = false
        controller.positionDirty = false

        if (data.resize) {
            original.width = 1920
            original.height = 1080
        } else {
            main.selectMonitor(smaller)
        }
        compare(controller.fixed_x, 1919)
        compare(controller.fixed_y, 1079)
        compare(controller.fixed_position_confirmed, false)

        main.selectMonitor(larger)

        compare(controller.fixed_x, 1919)
        compare(controller.fixed_y, 1079)
        compare(controller.fixed_position_confirmed, false)
        compare(controller.configDirty, false)
        compare(controller.positionDirty, false)
        compare(controller.savedMonitorIdentity, savedIdentity)
        compare(controller.savedFixedX, 3000)
        compare(controller.savedFixedY, 1500)

        if (data.action === "restore") {
            original.width = 3840
            original.height = 2160
            main.selectMonitor(original)
            compare(controller.fixed_x, 3000)
            compare(controller.fixed_y, 1500)
            compare(findChild(main, "xInput").value, 3000)
            compare(findChild(main, "yInput").value, 1500)
            compare(controller.savedMonitorIdentity, savedIdentity)
            // The restored point is the user's own choice, not a clamped one.
            compare(controller.fixed_position_confirmed, true)
            compare(controller.savedFixedX, 3000)
            compare(controller.savedFixedY, 1500)
        } else if (data.action === "confirm") {
            main.confirmMonitor()
        } else if (data.action === "pick") {
            // The picker needs a real QScreen; keep it inside the test display.
            main.selectMonitor(main.monitorOptions[0].screen)
            main.beginPicker(false)
            const picker = main.positionPicker
            verify(picker !== null)
            tryCompare(picker, "inputReady", true)
            tryCompare(picker, "active", true)
            mouseClick(picker.contentItem, 120, 180)
            keyClick(Qt.Key_Return)
            tryCompare(main, "positionPicker", null)
            compare(controller.fixed_x, 120)
            compare(controller.fixed_y, 180)
        } else {
            const input = findChild(main, data.action)
            input.contentItem.forceActiveFocus()
            keyClick(Qt.Key_A, Qt.ControlModifier)
            for (const digit of "123") keyClick(digit)
            compare(input.value, 123)
            compare(controller.fixed_position_confirmed, false)
        }

        compare(controller.positionDirty, true)
        compare(controller.savedFixedX, controller.fixed_x)
        compare(controller.savedFixedY, controller.fixed_y)
        // A deliberate choice allows later fitting monitor selections again.
        const chosenX = controller.fixed_x
        const chosenY = controller.fixed_y
        const next = createTemporaryObject(monitorComponent, main, { name: "D" })
        main.selectMonitor(next)
        compare(controller.fixed_position_confirmed, true)
        compare(controller.fixed_x, chosenX)
        compare(controller.fixed_y, chosenY)
        compare(controller.savedMonitorIdentity, controller.monitor_identity)
    }

    function test_returning_to_saved_monitor_confirms_saved_position() {
        const original = createTemporaryObject(monitorComponent, main, { name: "A" })
        const smaller = createTemporaryObject(monitorComponent, main,
            { name: "B", width: 1920, height: 1080 })
        controller.current_position = false
        main.selectMonitor(original)
        controller.fixed_x = 3000
        controller.fixed_y = 1500
        main.confirmMonitor()
        const savedIdentity = controller.savedMonitorIdentity

        main.selectMonitor(smaller)
        compare(controller.fixed_x, 1919)
        compare(controller.fixed_y, 1079)
        compare(controller.fixed_position_confirmed, false)

        // Returning restores the saved point unclamped; it needs no new confirmation.
        main.selectMonitor(original)
        compare(controller.fixed_x, 3000)
        compare(controller.fixed_y, 1500)
        compare(controller.fixed_position_confirmed, true)
        compare(controller.savedMonitorIdentity, savedIdentity)
        compare(controller.savedFixedX, 3000)
        compare(controller.savedFixedY, 1500)
    }

    function test_selecting_new_monitor_accepts_unclamped_position() {
        controller.current_position = false
        compare(controller.fixed_position_confirmed, false)
        const next = createTemporaryObject(monitorComponent, main, { name: "new-monitor" })

        main.selectMonitor(next)

        compare(controller.fixed_position_confirmed, true)
        compare(controller.positionDirty, true)
        compare(controller.savedFixedX, 80)
        compare(controller.savedFixedY, 150)
        compare(controller.savedMonitorIdentity, controller.monitor_identity)
    }

    function test_reselecting_saved_monitor_at_smaller_resolution_keeps_saved_coordinates() {
        const smaller = Qt.createQmlObject('import QtQuick; QtObject {'
            + 'property string name: "saved-monitor"; property string manufacturer: "";'
            + 'property string model: ""; property string serialNumber: "";'
            + 'property int width: 400; property int height: 300;'
            + 'property int virtualX: 0; property int virtualY: 0;'
            + 'property real devicePixelRatio: 1}', main)
        verify(smaller !== null)
        controller.current_position = false
        main.selectedMonitor = smaller
        controller.savedMonitorIdentity = controller.monitor_identity
        controller.savedFixedX = 300
        controller.savedFixedY = 200
        controller.fixed_x = 300
        controller.fixed_y = 200
        controller.fixed_position_confirmed = true
        smaller.width = 100
        smaller.height = 100
        compare(controller.fixed_position_confirmed, false)
        compare(controller.fixed_x, 99)
        compare(controller.fixed_y, 99)
        const savedIdentity = controller.savedMonitorIdentity
        main.selectMonitor(smaller)

        compare(controller.fixed_x, 99)
        compare(controller.fixed_y, 99)
        compare(controller.fixed_position_confirmed, false)
        compare(controller.positionDirty, false)
        compare(controller.savedFixedX, 300)
        compare(controller.savedFixedY, 200)
        compare(controller.savedMonitorIdentity, savedIdentity)
    }

    function test_mouse_wheel_cannot_replace_saved_position() {
        for (const name of ["xInput", "yInput", "monitorInput"])
            compare(findChild(main, name).wheelEnabled, false)
    }

    function test_save_error_dialog_keeps_choices_reachable() {
        controller.mark_settings_changed()
        controller.saveConfigSucceeds = false
        main.close()
        const dialog = findChild(main, "saveFailureDialog")
        tryCompare(dialog, "opened", true)
        for (const name of ["continueEditingButton", "discardSettingsButton"]) {
            const button = findChild(main, name)
            const right = button.mapToItem(dialog.contentItem, button.width, 0).x
            verify(right <= dialog.availableWidth, name + " ends at " + right)
        }
        const continueButton = findChild(main, "continueEditingButton")
        const discardButton = findChild(main, "discardSettingsButton")
        verify(discardButton.y >= continueButton.y + continueButton.height)
        // The header close button rejects the dialog.
        dialog.reject()
        tryCompare(dialog, "opened", false)
        compare(controller.initializeCount, 1)
    }

    function test_running_and_pending_runs_disable_settings() {
        for (const state of ["running", "busy"]) {
            controller[state] = true
            for (const name of ["intervalInput", "repeatInput", "xInput", "yInput", "monitorInput"])
                compare(findChild(main, name).enabled, false)
            controller[state] = false
        }
    }

    function test_close_saves_draft_and_keeps_window_open_on_write_error() {
        controller.interval_ms = 250
        controller.mark_settings_changed()
        controller.saveConfigSucceeds = false
        controller.running = true
        main.close()
        compare(controller.saveCount, 1)
        compare(controller.shutdownCount, 1)
        compare(controller.running, false)
        compare(main.visible, true)
        const dialog = findChild(main, "saveFailureDialog")
        verify(dialog !== null)
        tryCompare(dialog, "opened", true)
        keyClick(Qt.Key_Escape)
        compare(dialog.opened, true)

        controller.saveConfigSucceeds = true
        const continueEditing = findChild(main, "continueEditingButton")
        verify(continueEditing !== null)
        mouseClick(continueEditing)
        compare(controller.initializeCount, 1)
        compare(controller.hotkey_ready, false)
        compare(controller.hotkey_pending, true)
        compare(findChild(main, "startStopButton").enabled, false)
        main.close()
        compare(controller.saveCount, 2)
        compare(controller.shutdownCount, 2)
    }

    function test_discard_after_save_error_closes_without_retrying() {
        controller.mark_settings_changed()
        controller.saveConfigSucceeds = false
        main.close()
        const dialog = findChild(main, "saveFailureDialog")
        tryCompare(dialog, "opened", true)
        const discard = findChild(main, "discardSettingsButton")
        verify(discard !== null)
        mouseClick(discard)
        compare(controller.saveCount, 1)
        compare(controller.shutdownCount, 2)
    }

    function test_unchanged_window_does_not_rewrite_config() {
        main.close()
        compare(controller.saveCount, 0)
        compare(controller.shutdownCount, 1)
    }

    function test_edits_are_saved_on_close_without_click_start_data() {
        return [
            { tag: "mouse button", group: "mouseButtonInput", label: "Rechts", property: "mouse_button", value: 1 },
            { tag: "click type", group: "clickTypeInput", label: "Doppelklick", property: "click_type", value: 1 },
            { tag: "repeat mode", group: "repeatModeInput", label: "Klickzyklen", property: "repeat_until_stopped", value: false },
            { tag: "position mode", group: "positionModeInput", label: "Feste Position", property: "current_position", value: false },
            { tag: "repeat until stopped", group: "repeatModeInput", label: "Bis zum Stoppen", property: "repeat_until_stopped", initial: false, value: true },
            { tag: "cursor position", group: "positionModeInput", label: "Aktuelle Mauszeigerposition", property: "current_position", initial: false, value: true }
        ]
    }
    function test_edits_are_saved_on_close_without_click_start(data) {
        if (data.initial !== undefined) controller[data.property] = data.initial
        compare(controller.configDirty, false)
        const input = choice(data.group, data.label)
        input.forceActiveFocus()
        keyClick(Qt.Key_Space)
        compare(controller[data.property], data.value)
        compare(controller.configDirty, true)
        main.close()
        compare(controller.saveCount, 1)
        compare(controller.configDirty, false)
        compare(controller.running, false)
        compare(controller.busy, false)
    }

    function test_save_error_restores_minimized_window() {
        controller.mark_settings_changed()
        controller.saveConfigSucceeds = false
        main.showMinimized()
        tryCompare(main, "visibility", Window.Minimized)
        main.close()
        tryCompare(main, "visibility", Window.Windowed)
        const dialog = findChild(main, "saveFailureDialog")
        tryCompare(dialog, "opened", true)
        const discard = findChild(main, "discardSettingsButton")
        mouseClick(discard)
    }

    function test_screen_changes_keep_cursor_position_runs() {
        compare(controller.current_position, true)
        controller.monitor_width += 1
        controller.running = true
        main.handleScreensChanged()
        compare(controller.running, true)
        compare(findChild(main, "monitorStopMessage").visible, false)
    }

    function test_unchanged_monitor_keeps_fixed_position_runs() {
        controller.current_position = false
        controller.fixed_position_confirmed = true
        controller.running = true
        main.handleScreensChanged()
        compare(controller.running, true)
        compare(findChild(main, "monitorStopMessage").visible, false)
    }

    function test_changed_monitor_stops_fixed_position_runs_with_reason() {
        const message = findChild(main, "monitorStopMessage")
        for (const state of ["running", "busy"]) {
            controller.current_position = false
            controller.fixed_position_confirmed = true
            controller.monitor_width += 1
            controller[state] = true
            main.handleScreensChanged()
            compare(controller[state], false)
            tryCompare(message, "visible", true)
            controller[state] = true
            tryCompare(message, "visible", false)
            controller[state] = false
        }
    }

    function test_monitor_activation_does_not_confirm_clamped_coordinates() {
        controller.current_position = false
        const monitorInput = findChild(main, "monitorInput")
        const screen = main.monitorOptions[0].screen
        controller.fixed_x = screen.width + 100
        controller.fixed_position_confirmed = true
        monitorInput.activated(0)
        compare(controller.fixed_x, screen.width - 1)
        compare(controller.fixed_position_confirmed, false)

        monitorInput.activated(0)
        compare(controller.fixed_position_confirmed, false)
        main.confirmMonitor()
        compare(controller.fixed_position_confirmed, true)
    }

    function test_coordinate_inputs_have_distinct_accessible_names() {
        const x = findChild(main, "xInput")
        const y = findChild(main, "yInput")
        verify(x !== null)
        verify(y !== null)
        compare(x.Accessible.name, "X-Koordinate auf dem gewählten Bildschirm")
        compare(y.Accessible.name, "Y-Koordinate auf dem gewählten Bildschirm")
        compare(x.contentItem.Accessible.name, x.Accessible.name)
        compare(y.contentItem.Accessible.name, y.Accessible.name)
    }

    Component {
        id: otherWindowComponent
        Window { width: 100; height: 100; visible: true }
    }

    function test_later_launch_activates_main_window() {
        const other = createTemporaryObject(otherWindowComponent, null)
        other.requestActivate()
        tryCompare(other, "active", true)
        main.activateFromLauncher()
        tryCompare(main, "active", true)
    }

    function test_later_launch_returns_to_open_picker() {
        controller.current_position = false
        main.beginPicker(false)
        const picker = main.positionPicker
        verify(picker !== null)
        tryCompare(picker, "inputReady", true)
        tryCompare(picker, "active", true)
        const other = createTemporaryObject(otherWindowComponent, null)
        other.requestActivate()
        tryCompare(other, "active", true)
        main.activateFromLauncher()
        tryCompare(picker, "active", true)
        verify(!main.active)
        main.finishPicker()
        compare(main.positionPicker, null)
    }

    function test_position_selection_opens_without_capture_delay() {
        controller.current_position = false
        main.beginPicker(false)
        verify(main.positionPicker !== null)
        compare(main.positionPicker.visible, true)
        tryVerify(function() { return main.positionPicker && main.positionPicker.inputReady })
        main.finishPicker()
        compare(main.positionPicker, null)
    }

    function test_picked_position_uses_the_pickers_actual_monitor_data() {
        return [{ tag: "smaller monitor", width: 100, height: 100 },
                { tag: "same size monitor", width: 0, height: 0 }]
    }

    function test_picked_position_uses_the_pickers_actual_monitor(data) {
        controller.current_position = false
        main.beginPicker(false)
        const picker = main.positionPicker
        verify(picker !== null)
        tryCompare(picker, "inputReady", true)
        tryCompare(picker, "active", true)
        const actual = main.selectedMonitor
        // Reproduce a compositor placing the picker on a different output:
        // the selection and the actual fullscreen output no longer agree.
        const other = createTemporaryObject(monitorComponent, main, {
            name: "Other", virtualX: actual.virtualX + actual.width,
            width: data.width || actual.width, height: data.height || actual.height
        })
        main.selectMonitor(other)
        mouseClick(picker.contentItem, 120, 180)
        keyClick(Qt.Key_Return)
        tryCompare(main, "positionPicker", null)
        compare(main.selectedMonitor, actual)
        compare(controller.monitor_identity, controller.savedMonitorIdentity)
        compare(controller.monitor_x, actual.virtualX)
        compare(controller.monitor_y, actual.virtualY)
        compare(controller.monitor_width, actual.width)
        compare(controller.monitor_height, actual.height)
        compare(findChild(main, "monitorInput").currentIndex,
                main.monitorOptions.findIndex(option => option.screen === actual))
        compare(controller.fixed_x, 120)
        compare(controller.fixed_y, 180)
        compare(controller.savedFixedX, 120)
        compare(controller.savedFixedY, 180)
        verify(controller.configDirty)
        compare(findChild(main, "xInput").value, controller.fixed_x)
        compare(findChild(main, "yInput").value, controller.fixed_y)
        verify(controller.fixed_x < controller.monitor_width)
        verify(controller.fixed_y < controller.monitor_height)
        verify(controller.fixed_position_confirmed)
        verify(!controller.selecting_position)
        main.close()
        compare(controller.saveCount, 1)
        verify(!controller.configDirty)
    }

    function test_invalid_picker_result_preserves_saved_position_and_can_be_retried() {
        controller.current_position = false
        main.confirmMonitor()
        const saved = { x: controller.fixed_x, y: controller.fixed_y,
                        identity: controller.savedMonitorIdentity }
        main.beginPicker(false)
        const picker = main.positionPicker
        tryCompare(picker, "inputReady", true)
        // Deliberately inject an invalid result to exercise the defensive
        // receiver guard. Pointer input itself is clamped by setTarget().
        // Use confirm() so the failed attempt also closes the picker, as in UI.
        picker.targetX = picker.width + 5
        picker.targetY = 0
        picker.confirm()
        tryCompare(main, "positionPicker", null)
        verify(!controller.selecting_position)
        verify(!controller.fixed_position_confirmed)
        verify(controller.error_message.length > 0)
        compare(controller.fixed_x, saved.x)
        compare(controller.fixed_y, saved.y)
        compare(controller.savedMonitorIdentity, saved.identity)
        compare(controller.savedFixedX, saved.x)
        compare(controller.savedFixedY, saved.y)
        main.beginPicker(false)
        const retry = main.positionPicker
        verify(retry !== null)
        tryCompare(retry, "inputReady", true)
        tryCompare(retry, "active", true)
        mouseClick(retry.contentItem, 120, 180)
        keyClick(Qt.Key_Return)
        tryCompare(main, "positionPicker", null)
        compare(controller.error_message, "")
        verify(controller.fixed_position_confirmed)
        compare(controller.fixed_x, 120)
        compare(controller.fixed_y, 180)
        compare(controller.savedFixedX, 120)
        compare(controller.savedFixedY, 180)
    }

    function test_screen_change_cancels_open_picker_without_saving_its_point_data() {
        return [{ tag: "outputs changed", event: "outputs", invalidates: false },
                { tag: "size changed", event: "size", invalidates: true },
                { tag: "origin changed", event: "origin", invalidates: false }]
    }

    function test_screen_change_cancels_open_picker_without_saving_its_point(data) {
        controller.current_position = false
        main.confirmMonitor()
        const saved = { x: controller.fixed_x, y: controller.fixed_y,
                        identity: controller.savedMonitorIdentity }
        main.beginPicker(false)
        const picker = main.positionPicker
        tryCompare(picker, "inputReady", true)
        tryCompare(picker, "active", true)
        mouseClick(picker.contentItem, 120, 180)
        if (data.event === "outputs") main.handleScreensChanged()
        else main.handleMonitorGeometryChanged(data.invalidates)
        tryCompare(main, "positionPicker", null)
        verify(!controller.selecting_position)
        tryCompare(main, "active", true)
        compare(controller.fixed_x, saved.x)
        compare(controller.fixed_y, saved.y)
        compare(controller.savedFixedX, saved.x)
        compare(controller.savedFixedY, saved.y)
        compare(controller.savedMonitorIdentity, saved.identity)
        compare(controller.fixed_position_confirmed, !data.invalidates)
    }

    function test_rate_and_status_distinguish_cycles_from_clicks() {
        const rateLabel = findChild(main, "rateLabel")
        const statusLabel = findChild(main, "statusLabel")
        verify(rateLabel !== null)
        verify(statusLabel !== null)
        const twoAndHalf = Number(2.5).toLocaleString(Qt.locale(), 'f', 1)
        const nearOneCycle = Number(1.001).toLocaleString(Qt.locale(), 'f', 3)
        const nearTwoClicks = Number(2.002).toLocaleString(Qt.locale(), 'f', 3)
        const scenarios = [
            { interval: 10, type: 0, expected: "100 Zyklen/s · 100 Klicks/s" },
            { interval: 10, type: 1, expected: "100 Zyklen/s · 200 Klicks/s" },
            { interval: 1000, type: 0, expected: "1 Zyklus/s · 1 Klick/s" },
            { interval: 1000, type: 1, expected: "1 Zyklus/s · 2 Klicks/s" },
            { interval: 999, type: 0, expected: nearOneCycle + " Zyklen/s · " + nearOneCycle + " Klicks/s" },
            { interval: 999, type: 1, expected: nearOneCycle + " Zyklen/s · " + nearTwoClicks + " Klicks/s" },
            { interval: 5000, type: 0, expected: "1 Zyklus alle 5 s · 1 Klick alle 5 s" },
            { interval: 5000, type: 1, expected: "1 Zyklus alle 5 s · 2 Klicks alle 5 s" },
            { interval: 2500, type: 1, expected: "1 Zyklus alle " + twoAndHalf + " s · 2 Klicks alle " + twoAndHalf + " s" }
        ]
        for (const scenario of scenarios) {
            controller.interval_ms = scenario.interval
            controller.click_type = scenario.type
            tryCompare(rateLabel, "text", scenario.expected)
            controller.status = "Klicken aktiv"
            controller.running = true
            compare(statusLabel.text, "Status: Klicken aktiv · " + scenario.expected)
            controller.running = false
        }
    }

    function test_repeat_count_is_labeled_as_cycles() {
        compare(choice("repeatModeInput", "Klickzyklen").text, "Klickzyklen")
    }

    function test_hotkey_recovery_controls() {
        const retry = findChild(main, "hotkeyConfigureButton")
        const start = findChild(main, "startStopButton")
        const status = findChild(main, "hotkeyStatusLabel")
        verify(retry !== null)
        verify(start !== null)
        verify(status !== null)

        controller.hotkey_ready = false
        controller.hotkey_pending = false
        compare(retry.text, "Erneut versuchen")
        compare(status.text, "Nicht verfügbar")
        compare(retry.enabled, true)
        compare(start.enabled, false)

        controller.hotkey_pending = true
        compare(status.text, "Wird eingerichtet …")
        compare(retry.enabled, false)
        compare(start.enabled, false)

        controller.hotkey_ready = true
        controller.hotkey_configuring = true
        compare(status.text, "Dialog öffnet …")
        compare(retry.enabled, false)
        compare(start.enabled, false)

        controller.hotkey_pending = false
        controller.hotkey_configuring = false
        compare(status.text, "Pause")
        compare(retry.text, "Ändern …")
        compare(retry.enabled, true)
        compare(start.enabled, true)
    }

    function test_run_control_stays_visible_at_minimum_size_data() {
        return [
            { tag: "cursor-ready", fixed: false, running: false, busy: false, ready: true, pending: false },
            { tag: "fixed-ready", fixed: true, running: false, busy: false, ready: true, pending: false },
            { tag: "cursor-running", fixed: false, running: true, busy: false, ready: true, pending: false },
            { tag: "fixed-running", fixed: true, running: true, busy: false, ready: true, pending: false },
            { tag: "cursor-starting", fixed: false, running: false, busy: true, ready: false, pending: true },
            { tag: "fixed-starting", fixed: true, running: false, busy: true, ready: false, pending: true },
            // A shown send error must not take away Stop, even without a hotkey.
            { tag: "running-hotkey-unavailable", fixed: false, running: true, busy: false, ready: false, pending: false },
            { tag: "hotkey-unavailable", fixed: false, running: false, busy: false, ready: false, pending: false },
            { tag: "hotkey-pending", fixed: false, running: false, busy: false, ready: false, pending: true }
        ]
    }

    function test_run_control_stays_visible_at_minimum_size(data) {
        main.width = main.minimumWidth
        main.height = main.minimumHeight
        controller.current_position = !data.fixed
        controller.fixed_position_confirmed = false
        // Keep scrolling necessary even with smaller CI fonts and icon themes.
        controller.error_message = Array(16).join("Ein Portalfehler mit zusätzlicher Beschreibung. ")
        controller.status = data.busy ? "Warte auf Freigabe für Maussteuerung …" : (data.running ? "Klicken aktiv" : "Bereit")
        controller.hotkey_ready = data.ready
        controller.hotkey_pending = data.pending
        controller.busy = data.busy
        controller.running = data.running

        const footer = findChild(main, "runFooter")
        const button = findChild(main, "startStopButton")
        const status = findChild(main, "statusLabel")
        verify(footer !== null)
        verify(button !== null)
        verify(status !== null)
        tryCompare(findChild(main, "errorBanner"), "visible", true)
        tryCompare(main, "height", main.minimumHeight)
        wait(50)

        function inWindow(item) {
            const position = item.mapToItem(null, 0, 0)
            return position.x >= 0 && position.y >= 0
                && position.x + item.width <= main.width
                && position.y + item.height <= main.height
        }
        verify(button.visible && inWindow(button), "Run control must remain in the window")
        verify(status.visible && inWindow(status), "Run status must remain in the window")
        compare(button.enabled, data.running || data.busy || (data.ready && !data.pending))
        compare(button.text, data.running || data.busy ? "■  Stoppen" : "▶  Starten")
        compare(button.Accessible.name, data.running || data.busy ? "Stoppen" : "Starten")
        const expectedStatus = "Status: " + controller.status
            + (data.running ? " · " + main.rateDescription : "")
        compare(status.text, expectedStatus)
        // A click at the button's position must reach it, not the settings.
        const enabled = button.enabled
        const stopMode = data.running || data.busy
        mouseClick(button)
        compare(controller.stops, enabled && stopMode ? 1 : 0)
        compare(controller.button_starts, enabled && !stopMode ? 1 : 0)
        // The mock's stop() ends the run at once; restore it for the second click.
        controller.busy = data.busy
        controller.running = data.running

        const flickable = main.pageStack.currentItem.flickable
        verify(flickable.height > 0, "Long errors must leave room for the settings")
        verify(findChild(main, "messageHeader").height <= main.height / 3 + 1)
        tryVerify(function() { return flickable.contentHeight > flickable.height },
                  1000, "Settings must remain scrollable")
        flickable.contentY = flickable.contentHeight - flickable.height
        wait(50)
        verify(button.visible && inWindow(button), "Scrolling settings must not move the run control")
        verify(status.visible && inWindow(status), "Scrolling settings must not move the run status")
        mouseClick(button)
        compare(controller.stops, enabled && stopMode ? 2 : 0)
        compare(controller.button_starts, enabled && !stopMode ? 2 : 0)
    }

    function test_run_control_can_be_used_with_keyboard() {
        const button = findChild(main, "startStopButton")
        verify(button.activeFocusOnTab)
        button.forceActiveFocus()
        verify(button.activeFocus)
        keyClick(Qt.Key_Space)
        compare(controller.button_starts, 1)
    }

    function test_run_control_follows_settings_in_focus_and_reading_order() {
        const lastSetting = findChild(main, "hotkeyConfigureButton")
        const button = findChild(main, "startStopButton")
        const footer = findChild(main, "runFooter")
        verify(lastSetting.enabled)
        lastSetting.forceActiveFocus(Qt.TabFocusReason)
        verify(lastSetting.activeFocus)
        keyClick(Qt.Key_Tab)
        verify(button.activeFocus, "Tab must move from the last setting to the run control")
        keyClick(Qt.Key_Backtab, Qt.ShiftModifier)
        verify(lastSetting.activeFocus, "Shift+Tab must return from the run control to the settings")

        // Screen readers list sibling items in child order, so the footer
        // must follow the settings page.
        let page = main.pageStack
        while (page && page.parent !== footer.parent) page = page.parent
        verify(page, "Footer and settings must share a parent item")
        const siblings = footer.parent.children
        verify(siblings.indexOf(page) < siblings.indexOf(footer),
               "The footer must follow the settings in reading order")
    }

    function test_button_shows_and_cancels_countdown() {
        const button = findChild(main, "startStopButton")
        verify(button !== null)
        controller.hotkey_ready = true
        controller.hotkey_pending = false
        compare(button.text, "▶  Starten")
        compare(button.Accessible.name, "Starten")
        mouseClick(button)
        compare(controller.button_starts, 1)
        compare(controller.stops, 0)

        // Waiting for the Wayland permission: pending, but no countdown yet.
        controller.busy = true
        compare(button.text, "■  Stoppen")
        compare(button.Accessible.name, "Stoppen")

        // The countdown stays cancellable even if the hotkey becomes unavailable.
        controller.countdown_remaining = 3
        compare(button.text, "■  Start abbrechen (3)")
        compare(button.Accessible.name, "Start abbrechen, Klicken beginnt in 3 Sekunden")
        controller.countdown_remaining = 1
        compare(button.text, "■  Start abbrechen (1)")
        compare(button.Accessible.name, "Start abbrechen, Klicken beginnt in 1 Sekunde")
        controller.hotkey_ready = false
        compare(button.enabled, true)
        mouseClick(button)
        compare(controller.stops, 1)
        compare(controller.countdown_remaining, 0)
        compare(controller.button_starts, 1)
        controller.hotkey_ready = true
        compare(button.text, "▶  Starten")
        compare(button.Accessible.name, "Starten")

        // A countdown value without a pending start is never shown.
        controller.countdown_remaining = 2
        compare(button.text, "▶  Starten")
        compare(button.Accessible.name, "Starten")
        controller.countdown_remaining = 0

        // A running click loop is stopped, never restarted with a countdown.
        controller.running = true
        compare(button.text, "■  Stoppen")
        mouseClick(button)
        compare(controller.stops, 2)
        compare(controller.button_starts, 1)
    }
}
