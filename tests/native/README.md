# Native KDE-Regressionstests

Aus dem Projektverzeichnis, in einer entsperrten **deutschen Plasma-Wayland-Sitzung
mit genau einem Monitor**. Benötigt werden Python mit PySide6, dbus-python,
PyGObject/AT-SPI, `qdbus6` und das gebaute Release-Programm mit eingerichteter
[Desktop-Integration](../../README.md#desktop-integration). Die `.desktop`-Datei allein
genügt nicht, das Portal muss auch ihr `Exec` auflösen können. `run.py` und damit
auch `benchmark.py` melden sich deshalb vorab mit einer Wegwerf-Verbindung bei der
Portal-Registry an und brechen bei einer Ablehnung mit Ursache und Abhilfe ab,
bevor ein Fenster startet.
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
- Endet die App mit Fehlercode oder durch ein Signal, etwa bei einem Absturz beim
  Schließen, schlägt der Lauf fehl, auch wenn das Szenario bestanden meldet.

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

## Leistungsmessung

`benchmark.py` misst Klickrate, Streuung und Stopplatenz des gebauten Programms
mit demselben Testziel, Wächter, eigener Konfiguration und den regulären
KDE-Freigaben wie die Regressionstests:

```bash
cargo build --locked --release
python tests/native/benchmark.py                 # 1×/2× bei 100, 20 und 10 ms
python tests/native/benchmark.py --long          # je 60 s bei 10 ms
python tests/native/benchmark.py --modes double --intervals 10 --duration 20
python tests/native/benchmark.py --rates 100,250,500   # geplante Klicks/s
```

Weitere Optionen zeigt `--help`. Fragt KDE beim ersten Start nach dem globalen
Tastenkürzel, diesen Dialog selbst bestätigen; der Benchmark wartet bis zu 120 s
auf `Bereit` und bestätigt erst danach die Fernsteuerung für den Eingabetreiber.
Die Freigabe der App beim ersten Klicklauf bestätigt ein Aufwärmlauf mit drei
Klicks; er wird getrennt ausgegeben. Danach folgt pro Modus
und Intervall ein gezählter Lauf, dessen Zyklenzahl die ideale Dauer ergibt
(`--duration`, Standard 5 s). `--rates` gibt statt der Intervalle geplante
einzelne Klicks/s vor; Doppelklicks laufen dann mit dem doppelten Intervall, bei
500 Klicks/s also 2 ms (1×) und 4 ms (2×). Intervalle unter der Grenze der App
(2 ms bzw. 4 ms) werden abgelehnt. Anschließend wird je dreimal per Schaltfläche und
per Tastenkürzel nach 1 s Klicken gestoppt; danach dürfen 1 s lang (`--quiet`)
keine Ereignisse mehr ankommen.

Jeder Lauf prüft Anzahl, Taste, Koordinaten (80, 400) und vollständige
Press-/Release-Paare; `MouseButtonDblClick` zählt als Beginn eines Klicks.
Ausgegeben werden geplante (`Soll`) und erreichte (`Ist`) Zyklen/s und einzelne
Klicks/s, die erreichten nach
`(Zyklen - 1) / (letzter - erster Zyklusbeginn)`, Median/P95/P99/Maximum der
Abstände der Zyklusbeginne (nächster Rang), die Zeit vom Start bis zum ersten
Ereignis, CPU-Anteil von App, Testziel und Beobachter sowie RSS und Spitzen-RSS
der App. Dazu kommen Commit, Buildprofil und Änderungszeit des Programms, System,
CPU, Qt-Versionen von App und Testziel, KWin und Bildschirm.

Messgrenzen: Zeitpunkte sind Empfangszeiten am Testziel (`CLOCK_MONOTONIC`) und
enthalten Portal, Compositor und Qt-Ereigniszustellung. Die feste Position
verwendet keinen Start-Countdown; Freigaben fallen nur in den Aufwärmlauf. Die
Zyklenstatistik beginnt beim ersten empfangenen Ereignis. Während eines Laufs
liest der Beobachter alle 5 ms nur neue Zeilen des Ereignisprotokolls; Status und
Start/Stopp-Schaltfläche werden einmal über AT-SPI aufgelöst und danach direkt
abgefragt. Die CPU-Anteile gelten zwischen dem ersten und letzten beobachteten
Ereignis, weichen also um bis zu einen Abfrageschritt ab. Der Aufwand von
AT-SPI-Registry, Compositor und Portal ist nicht enthalten. Stoppwerte zählen ab
dem Absenden der Anforderung: Aufruf (AT-SPI-Aktion bzw. Tastendruck über das
Portal), sichtbarer Status `Gestoppt` und letztes danach empfangenes Ereignis.

Alle Zeiten sind Messwerte dieses Geräts und dieser Sitzung, keine
Abnahmegrenzen; Fehler entstehen nur bei falschen oder fehlenden Ereignissen.
Der Benchmark läuft deshalb nicht in der CI. Am Ende beendet `run.py` alle
eigenen Prozesse und entfernt das temporäre Verzeichnis; `--keep-artifacts`
behält Protokolle und Rohereignisse.

`scenarios.py` enthält Hilfsfunktionen für weitere beaufsichtigte Fälle; es ist
kein eigenständiger Teststarter.
