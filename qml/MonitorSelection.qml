import QtQuick

QtObject {
    // Qt supplies these fields from the current system's display metadata.
    function modelName(screen) {
        const model = String(screen.model || "").trim()
        return model !== String(screen.name || "").trim() ? model : ""
    }

    function baseName(screen) {
        const model = modelName(screen)
        const manufacturer = String(screen.manufacturer || "").trim()
        if (!model) return manufacturer ? qsTr("%1 screen").arg(manufacturer) : qsTr("Screen")
        if (!manufacturer || model.toLowerCase().startsWith(manufacturer.toLowerCase())) return model
        return manufacturer + " " + model
    }

    // Keep models readable; connectors only disambiguate duplicates or missing metadata.
    function displayName(screen, screens) {
        if (!screen) return qsTr("No screen available")
        const name = baseName(screen)
        let ambiguous = false
        let index = -1
        for (let i = 0; i < screens.length; ++i) {
            if (screens[i] === screen) index = i
            else if (baseName(screens[i]) === name) ambiguous = true
        }
        if (modelName(screen) && !ambiguous) return name
        const connector = String(screen.name || "").trim()
        return connector ? qsTr("%1 (%2)").arg(name).arg(connector)
            : index >= 0 ? qsTr("%1 %2").arg(name).arg(index + 1) : name
    }

    // Include the connector and hardware identity; geometry changes require confirmation.
    function identity(screen) {
        return JSON.stringify([screen.name, screen.manufacturer, screen.model,
            screen.serialNumber, screen.width, screen.height, screen.devicePixelRatio])
    }

    // Restore only an unambiguous match, never the first of several similar monitors.
    function restoreIndex(screens, savedIdentity) {
        if (!savedIdentity) return -1
        let match = -1
        for (let i = 0; i < screens.length; ++i) {
            if (identity(screens[i]) !== savedIdentity) continue
            if (match !== -1) return -1
            match = i
        }
        return match
    }
}
