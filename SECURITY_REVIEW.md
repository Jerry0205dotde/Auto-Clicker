# Security Review

Stand: 2026-10-27, Arbeitskopie nach Version 0.1.3 (Portal-Anmeldung, Mehrmonitorzuordnung und Einzelinstanz; frühere native Geräteprüfung siehe Testbericht zu 0.1.1)

## 1. Benötigte Berechtigungen

- Globale Shortcut-Aktion: erforderlich als sofort erreichbarer Start/Stop-Failsafe.
- RemoteDesktop-Gerät `POINTER`: erforderlich, um linke, rechte oder mittlere Button-Ereignisse zu emulieren.
- Monitor-Streams als ScreenCast-Koordinatenreferenz: nur bei fester Position erforderlich. Die KDE-Freigabe umfasst dabei alle Ausgänge; Klickmeister liest ausschließlich ihre Metadaten und verwendet nur den eindeutig passenden Stream für die Mauspositionierung.

Nicht angefordert werden `KEYBOARD`, `TOUCHSCREEN`, Screenshots, Clipboard, Kamera, Mikrofon, Dateien, Standort, Benachrichtigungen, Hintergrundausführung oder Netzwerk.

## 2. Verwendete D-Bus-/Portal-Schnittstellen

- `org.freedesktop.portal.GlobalShortcuts`: `CreateSession`, `BindShortcuts`, `ConfigureShortcuts`, `Activated`, `ShortcutsChanged` und `Session.Close`.
- `org.freedesktop.portal.RemoteDesktop`: `CreateSession`, `SelectDevices`, `Start`, `ConnectToEIS` und `Session.Close`. Nur wenn `ConnectToEIS` scheitert, etwa bei einem Portal vor Version 2, stattdessen `NotifyPointerButton` und optional `NotifyPointerMotionAbsolute`.
- `org.freedesktop.portal.ScreenCast`: nur `SelectSources` auf derselben RemoteDesktop-Sitzung. `OpenPipeWireRemote` wird nicht aufgerufen.

Über den Socket aus `ConnectToEIS` spricht Klickmeister das libei-Protokoll als Sender. Es bindet nur `ei_button` und dazu `ei_pointer` (Mauszeigerposition) bzw. `ei_pointer_absolute` (feste Position), denn KWin legt für eine Taste allein kein Gerät an. Relative Bewegungen sendet es nie; bei fester Position bewegt es den Zeiger nur innerhalb der Region, deren `mapping_id` zum gewählten Monitor-Stream gehört. Tastatur, Touch, Scrollen und Text werden nicht gebunden. Hat KWin den Socket übergeben, sendet dieselbe Sitzung keine `Notify*`-Aufrufe mehr; scheitert die libei-Einrichtung danach, bricht der Start ab.

Exportiert wird nur `org.freedesktop.Application.Activate` unter dem Busnamen `io.github.jerry0205.klickmeister` (Pfad `/io/github/jerry0205/klickmeister`). Die Methode zeigt lediglich das vorhandene Fenster an; das optionale `activation-token` wird an xdg-activation weitergereicht, alle anderen Plattformdaten werden ignoriert. Jeder Teilnehmer des Session-Busses kann sie aufrufen, also nur Prozesse derselben Benutzersitzung. Sie startet, stoppt und ändert nichts.

Sitzungen verwenden `PersistMode::Application`: keine anwendungsseitig gespeicherten Restore-Tokens und keine dauerhafte Berechtigung nach Prozessende.

Die RemoteDesktop-Sitzung bleibt nach einem Lauf höchstens 60 Sekunden ungenutzt bestehen, damit ein erneuter Start ohne Dialog auskommt. Solange zeigt KDE das Statussymbol „Fernsteuerung“, und bei fester Position bleiben die Monitor-Streams aller Ausgänge freigegeben. Danach schließt der Worker die Sitzung samt libei-Verbindung. „Beenden“ im Statussymbol schließt sie sofort; der Worker verwirft sie dann und fordert beim nächsten Start eine neue an. Beendet KWin nur die libei-Verbindung, gilt das ebenso als Ende der Freigabe.

## 3. Netzwerkzugriffe

Keine. Der Produktionscode enthält keine HTTP-, TCP-, UDP- oder DNS-API. D-Bus nutzt ausschließlich den lokalen Unix-Session-Bus. Cargo kann beim Bauen checksummengeprüften Quellcode beziehen; das ist kein Laufzeitverhalten der Anwendung.

## 4. Gespeicherte Daten

Nur `config.toml` im XDG-Konfigurationsverzeichnis der Anwendung. Gespeichert werden Intervall, Maustaste, Klicktyp, Wiederholungsmodus/-zahl, Positionsmodus, X/Y, Monitoridentität einschließlich Geometrie/Skalierung und sichtbare Hotkeybeschreibung. Der Austausch erfolgt über eine temporäre Datei im selben Verzeichnis und `rename`. Auf Unix wird die Datei mit Modus 0600 erstellt. Konnte eine vorhandene Datei beim Start nicht gelesen oder geparst werden, wird sie vor dem ersten Speichern geänderter Einstellungen als `config.toml.invalid` (bei Bedarf nummeriert) daneben neu angelegt, ohne bestehende Dateien oder Links zu überschreiben; eine bloße Hotkey-Rückmeldung von KDE löst dann keinen Schreibvorgang aus.

Keine Eingaben, Klickhistorien, Fenstertitel, Prozessinformationen, Clipboard-Inhalte, Kennwörter oder Portal-Restore-Tokens werden gespeichert.

## 5. Threading-Modell

Der Qt-Main-Thread besitzt die GUI. Genau ein aktiver Rust-Workerthread besitzt Zustandsautomat, Scheduler und Portalobjekte. Ein beim Schließen nicht rechtzeitig beendeter Worker hat seine Schleife bereits verlassen; er klickt nicht mehr und schließt nur noch seine eigenen Sitzungen. Ein begrenzter Kanal (16 Befehle) verhindert unbeschränktes Anwachsen. Stop und Shutdown verwenden ein priorisiertes Ein-Wert-Signal und kommen auch bei vollem Kanal an; vor einem Stop eingereihte Startbefehle werden verworfen. Zustandsupdates gelangen über die threadsichere CXX-Qt-Queue in den Qt-Event-Loop. Ein weiterer Thread hält nur die D-Bus-Verbindung der Einzelinstanzsperre. Er reicht Aktivierungsanfragen in den Qt-Event-Loop und berührt weder Worker noch Portale.

Alle Startpfade verwenden denselben `StateMachine`. `Starting` und `Clicking` weisen weitere Startbefehle ab. Es existiert höchstens ein `ActiveRun` und eine Start-Aufgabe. Abgebrochene Starts werden verworfen. Der Countdown eines Button-Starts beginnt erst nach der Wayland-Freigabe und gehört noch zu `Starting`; erst danach folgt `Clicking`. Stop, Hotkey, Fehler und der Widerruf der Freigabe verwerfen ihn zusammen mit dem Start.

## 6. Failsafe-Verhalten

- Ohne bestätigten globalen Hotkey wird Start abgewiesen.
- Hotkey und Stop-Button setzen den einzigen aktiven Run auf `Stopped` und entfernen seinen Termin.
- Tastendrücke, die xdg-desktop-portal vor seiner Antwort auf die Abfrage nach einem Runende weitergeleitet hat, starten danach keinen neuen Run, auch wenn sie sich hinter einem verzögerten Klick gestaut haben; beenden können sie einen Run weiterhin. Nach jedem Runende liest der Worker dazu über die Hotkey-Verbindung eine Portal-Eigenschaft beim eindeutigen Busnamen der Portal-Verbindung, die die Hotkey-Signale sendet. Wegen der D-Bus-Reihenfolge gilt jedes vor der Antwort empfangene `Activated`-Signal als alt. Es zählt nur eine Antwort dieser Verbindung, auch eine Fehlerantwort; Fehler des Bus-Daemons lassen alle Tastendrücke alt, bis eine erneute Abfrage beantwortet wird. Zur verbleibenden Lücke siehe Abschnitt 7.
- Fenster-Schließen sperrt zuerst Starts, auch über den Hotkey, und stoppt einen Lauf oder ausstehenden Start; danach wird gespeichert. Die Sperre gilt bis zum Ende des Schließversuchs einschließlich Fehlerdialog. Bei einem Speicherfehler bleiben Worker und Hotkey-Sitzung bestehen; „Zurück zur Anwendung“ hebt die Sperre auf und veraltet mit einem weiteren Stop alle Startanfragen aus der Zwischenzeit. Erst nach erfolgreichem Speichern oder „Ohne Speichern schließen“ ruft das Schließen synchron `shutdown` auf. Es fordert das Ende des Workers an und wartet höchstens 5 Sekunden darauf. Der Worker klickt danach nicht mehr, bricht ausstehende RemoteDesktop- und Hotkey-Anfragen kooperativ ab und schließt beide Portal-Sitzungen selbst. Jede Anfrage besitzt ihre D-Bus-Verbindung, die nach begrenztem `Session.Close` ebenfalls getrennt wird. Einen Stop bestätigt der Worker erst nach dieser Bereinigung. Endet der Worker nicht innerhalb der 5 Sekunden, räumt er im Hintergrund zu Ende auf, und der Controller verwirft seine späten Ereignisse über die Worker-Epoche.
- Ein Prozessende trennt zusätzlich automatisch den D-Bus-Client; es gibt keinen separaten Clickerprozess.
- Pro Session-Bus läuft nur eine Instanz. Sonst erhielten mehrere GlobalShortcuts-Sitzungen derselben App-ID jeden Tastendruck, und ein Druck könnte eine Instanz stoppen und eine andere starten. Die Sperre ist der D-Bus-Name `io.github.jerry0205.klickmeister`; ein weiterer Start zeigt das vorhandene Fenster an und beendet sich, bevor er eine Portalsitzung anlegt. Der Name endet mit dem Prozess und hinterlässt nach einem Absturz keine Sperre.
- Über libei gehen Druck und Loslassen eines Zyklus zusammen an KWin; was der Socket nicht sofort aufnimmt, bleibt gepuffert und wird nachgesendet. Bestätigt KWin den Zyklus nicht innerhalb von 250 ms über `ei_connection.sync`, geht der Scheduler in den Fehlerzustand, und der Worker schließt Sitzung und libei-Verbindung.
- Über die D-Bus-Notify-Methoden wird nach erfolgreichem Button-Press immer ein Release versucht. Schlägt Release fehl, folgt ein zweiter Best-Effort-Release und der Scheduler geht in Fehlerzustand.
- Verpasste Timings werden nicht nachgeholt; dadurch entsteht kein Event-Burst.

## 7. Bekannte Einschränkungen

- Ein echtes globales Auslesen der Cursorposition ist unter Wayland absichtlich nicht möglich. Der Picker nutzt ein eigenes Vollbildfenster.
- Für absolute Positionen müssen Position und logische Größe genau einen Monitor-Stream identifizieren. Fehlende Metadaten und gespiegelte Ausgänge mit identischer Geometrie verhindern den Start. Displayänderungen können die Sitzung ungültig machen und führen dann zum Stop mit Fehlermeldung.
- Die Portal-Dialoge für Tastenkürzel und Maussteuerung hängen am Hauptfenster, das Klickmeister über xdg-foreign exportiert (`wayland:<handle>`). Steht nach einer Sekunde noch kein Handle bereit oder schlägt der Export fehl, erscheint der Dialog wie früher als eigenes KWin-Fenster, etwa ohne Wayland-Sitzung.
- Der Positionswähler fragt keine Bildschirmaufnahme an. Das Screenshot-Portal liefert zu Aufnahmen keine verlässliche Ausschnittgeometrie.
- Die Abfrage nach einem Runende erfasst nur Tastendrücke, die xdg-desktop-portal bis zu seiner Antwort schon empfangen und weitergeleitet hat. Davor liegen KWin, kglobalaccel und xdg-desktop-portal-kde. Hängt eine dieser Komponenten, während der Nutzer den Hotkey drückt und danach auf Stopp klickt, erreichen diese Tastendrücke das Portal erst nach der Antwort und gelten als neu. Sie wirken dann wie frische Betätigungen: Im Ruhezustand fordert jede einen Start an, während eines Runs stoppt jede ihn. Ein einzelner solcher Tastendruck, allgemein eine ungerade Anzahl, kann den Klicker so wieder starten; erneutes Drücken des Hotkeys oder die Stop-Schaltfläche beenden ihn wie gewohnt. Wie lange ein solcher Hänger dauert, ist nicht begrenzt. Eine zeitliche Startsperre nach einem Stop ist deshalb bewusst nicht eingebaut: Sie würde lange Hänger nicht abdecken und echte Tastendrücke verwerfen.
- `SIGKILL` verhindert anwendungsseitiges RAII-Cleanup; der D-Bus-Verbindungsabbruch beendet die compositorseitige Sitzung dennoch.
- Klicks laufen über libei, sofern das Portal `ConnectToEIS` anbietet; die Protokollimplementierung `reis` ist reines Rust ohne native Bibliothek. Nur ohne `ConnectToEIS` bleiben die D-Bus-Notify-Methoden mit einem Aufruf je Ereignis. Das Limit von 500 einzelnen Klicks pro Sekunde (Abschnitt 9) begrenzt die Last in beiden Fällen; bei so kurzen Intervallen können je nach Portal, Compositor und Rechner weniger Klicks ankommen als geplant.

## 8. Dependency-Liste

Direkt: `ashpd`, `cxx`, `cxx-qt`, `cxx-qt-lib`, `futures-util`, `reis`, `rustix`, `serde`, `thiserror`, `tokio`, `toml`; Build: `cxx-qt-build`. `reis` (MIT) implementiert das libei-Protokoll in reinem Rust mit `#![forbid(unsafe_code)]` und bringt nur `log` als neue transitive Abhängigkeit mit. `rustix` war bereits transitive Abhängigkeit und liefert die monotone Uhr für libei-Zeitstempel. Die vollständige transitive Liste ist mit `cargo tree --locked` reproduzierbar. Qt/Kirigami, KWindowSystem und xdg-desktop-portal-kde kommen aus den signierten Arch-Repositories.

`cargo tree --locked` wurde für diesen Stand erfolgreich ausgeführt. Der dokumentierte Prüfbefehl ist `cargo deny check advisories bans licenses sources` mit den Regeln aus `deny.toml`. Der Workflow „Dependencies“ führt ihn bei Pull Requests und wöchentlich aus; Dependabot schlägt Updates für Cargo und GitHub Actions vor.

CXX-Qt erzeugt die notwendige Qt-FFI und enthält intern `unsafe`; der handgeschriebene Scheduler-, Konfigurations- und Portalcode enthält keine `unsafe`-Blöcke. Die beiden `unsafe extern "C++"`-Deklarationen in `controller.rs` und `qml_runtime.rs` sind ausschließlich typisierte CXX-Brücken zu Qt. `build.rs` enthält einen `unsafe`-Block, weil CXX-Qt jeden Zugriff auf seinen internen `cc::Build` so kennzeichnet; er fügt nur die Include-Verzeichnisse von KWindowSystem hinzu. Der kleine C++-Helfer prüft, ob QML ein Hauptfenster geladen hat, und reicht Aktivierungsanfragen weiterer Starts über `QMetaObject::invokeMethod` in den Qt-Thread. Dort setzt er `XDG_ACTIVATION_TOKEN` nur für den Aufruf von `requestActivate()` und entfernt die Variable danach wieder, wie es auch `KDBusService` tut. Außerdem exportiert er das Hauptfenster mit `KWaylandExtras::exportToplevel` und gibt nur den erhaltenen Handle-Text an Rust weiter.

## 9. Mögliche Sicherheitsrisiken

- Pointer-Steuerung ist eine wirkungsvolle Berechtigung. Ein Fehler könnte in der aktiven Sitzung auf falsche Oberflächen klicken. Begrenzung, Portalbestätigung, Einzel-Scheduler und mehrere Stopwege reduzieren das Risiko.
- Eine kompromittierte Portal-/Compositorimplementierung liegt außerhalb der Vertrauensgrenze der Anwendung.
- Bei fester Position stellt das Portal prinzipiell einen Streamhandle bereit. Klickmeister öffnet ihn nicht; eine spätere Codeänderung an diesem Punkt muss erneut geprüft werden.
- Abhängigkeiten bilden Supply-Chain-Risiken. `Cargo.lock`, Arch-Paketsignaturen, `cargo deny` und Review von Dependency-Änderungen sind die vorgesehenen Kontrollen.
- Doppelklick erzeugt pro begrenztem Scheduler-Tick zwei Press/Release-Paare. Die Zyklusrate ist so begrenzt, dass höchstens 500 einzelne Klicks pro Sekunde geplant werden (Einzelklick 500, Doppelklick 250 Zyklen/s). Es gibt weiterhin keine ungebremste Schleife.

## Review-Checkliste

- Netzwerk-APIs im Produktionscode: keine gefunden.
- Shellausführung oder Befehlszusammensetzung: keine gefunden.
- Root, sudo, `/dev/uinput`: keine Verwendung.
- systemd, Autostart, Daemon oder Prozess-Fork/-Spawn: keine Verwendung. Die beiden eigenen `std::thread`s, der Worker (`klickmeister-worker`) und die Einzelinstanz-Verwaltung (`klickmeister-instance`), bleiben Bestandteil desselben Prozesses.
- Fremdprozesse, Clipboard, Tastaturmitschnitt: keine Verwendung.
- Busy-Wait oder Intervall 0: durch Design und Validierung ausgeschlossen.
- Unbegrenzte Queue oder parallele Click-Loops: ausgeschlossen.
