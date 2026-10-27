# Native KDE-Regressionstests

Aus dem Projektverzeichnis, in einer entsperrten **deutschen Plasma-Wayland-Sitzung
mit genau einem Monitor**. Benötigt werden Python mit PySide6, dbus-python,
PyGObject/AT-SPI, `qdbus6` und das gebaute Release-Programm.
Diese Tests steuern den sichtbaren Desktop und bestätigen die eigenen KDE-Dialoge.
Währenddessen den Desktop nicht anderweitig bedienen und keine andere Klickmeister-
Instanz starten; läuft bereits eine, zeigt der Teststart nur deren Fenster an und endet. Globale Tastenkürzel dürfen nicht parallel umkonfiguriert werden.

```bash
cargo build --locked --release
python -m unittest discover -s tests/native -p 'test_*.py'
python tests/native/run.py tests/native/portal_cases.py
```

`run.py` erstellt ein privates Artefaktverzeichnis samt eigener App-Konfiguration.
Die Monitoridentität darin entspricht byte-genau `JSON.stringify` aus
`qml/MonitorSelection.qml`, auch bei gebrochener Skalierung; `test_run.py` prüft das.
Der Pfad wird ausgegeben; Ergebnisse, Mausereignisse und Prozess-IDs bleiben
dort erhalten. Es wird keine Screenshot-Freigabe angefragt. `portal_cases.py`
bedient den RemoteDesktop-Dialog über AT-SPI. KDE 6.7 meldet auf diesem Gerät
leere zugängliche Buttonnamen; nur bei
exakt passendem Dialog und Buttonanzahl wird die visuell geprüfte Reihenfolge
verwendet. Andere Sprachen/Layouts müssen vor Wiederverwendung angepasst werden.

Die Tests prüfen Positionswahl ohne Screenshot-Dialog, Klicks und Stop
nach der Positionswahl sowie reguläres App-Schließen.
Zusätzlich laufen alle drei Maustasten mit Einzel-/Doppelklick; Optionen werden
über sichtbare Auswahlknöpfe gewählt und als ausgewählt geprüft. Jeder neue Picker wird
bei (80, 400) fixiert und vor dem Start bestätigt.

Die aus den temporären Hilfsskripten übernommenen Korrekturen sind fest enthalten:

- Das Vollbild-Klickziel besitzt keinen Ablauf-Timer und ignoriert Fenster-Schließen
  einschließlich Alt+F4. Das reguläre Aufräumen beendet zuerst alle Eingabesender,
  dann das Testziel.
- App-Schließen adressiert das Hauptfenster anhand der vom Starter erzeugten PID.
- Der Wächter überwacht das Ziel mit Linux-pidfd und zusätzlich seine Sichtbarkeit.
  Bei Zielausfall, fehlender Antwort, Szenariofehler oder Zeitüberschreitung wird
  die eigene Test-App ohne D-Bus-/Fokusabhängigkeit beendet. Alle gestarteten
  Prozesse besitzen eigene Prozessgruppen; die Bereinigung erfasst auch deren
  Kindprozesse und funktioniert nach dem Ende des Gruppenleiters. Ein Prozessausfall weckt
  den Wächter sofort; Sichtbarkeit wird alle 250 ms mit 100-ms-Antwortgrenze geprüft.
  Dies ist keine Echtzeitgarantie und erkennt keine Überdeckung durch fremde Fenster.
- Wächtertests verwenden einen schreibenden Ersatzprozess, um einen laufenden
  Erzeuger ohne reale Klicks kontrolliert ausfallen zu lassen. Danach dürfen keine
  Ereignisse hinzukommen. Abbruch, App-Ende und Zielende sind getrennt abgesichert.

Ein zusätzlicher nativer Ausfalltest beendet das echte Testziel **bei ruhendem
Klicker**. Der folgende Aufruf muss mit Fehlerstatus und `Click target exited`
enden. Alle PIDs aus `pids.json` müssen danach beendet sein:

```bash
python tests/native/run.py tests/native/target_loss.py
```

Verworfene Läufe zählen nicht als bestandene Nachweise. Bei Änderungen an KDE,
Monitoranordnung oder Sprache zuerst die sichtbaren Dialoge und Koordinaten prüfen.
Der automatisierte Starter `run.py` verwendet weiterhin genau einen Monitor.
Für beaufsichtigte Mehrmonitorprüfungen kann `target.py` separat pro Ausgang
mit eigenem `KLICKMEISTER_NATIVE_DIR` gestartet werden. Die optionale Variable
`KLICKMEISTER_NATIVE_SCREEN` wählt den exakten Qt-Bildschirmnamen (zum Beispiel
`X11-1`); ohne diese Variable gilt der primäre Bildschirm. Das Ziel zeigt den
Bildschirmnamen und die zuletzt tatsächlich empfangenen lokalen Koordinaten an.
Ereignisse enthalten zusätzlich globale Koordinaten.

`scenarios.py` enthält Hilfsfunktionen für weitere beaufsichtigte Fälle; es ist
kein eigenständiger Teststarter.
