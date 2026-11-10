# Klickmeister

Automatische Mausklicks für KDE Plasma.

Klickmeister übernimmt wiederholte Mausklicks unter KDE Plasma 6 auf Wayland. Wähle Position, Klickintervall und Wiederholungen. Ein globales Tastenkürzel startet und stoppt das Klicken.

![Klickmeister mit festem Punkt und erreichbaren Start-/Stopp-Knöpfen](docs/screenshot.png)

## Funktionen

- Linke, rechte oder mittlere Maustaste
- Einzel- oder Doppelklick
- Aktuelle Mauszeigerposition oder feste Position auf einem ausgewählten Bildschirm
- Positionsauswahl direkt auf dem Desktop, mit Feineinstellung per Pfeiltasten
- Eine festgelegte Anzahl von Klickzyklen oder Klicken bis zum Stopp
- Klickintervalle von 2 Millisekunden (Doppelklick: 4 Millisekunden) bis 24 Stunden

Ein Klickzyklus entspricht einem Einzel- oder Doppelklick. Drei Zyklen erzeugen drei Einzelklicks oder drei Doppelklicks, also sechs einzelne Klicks. Klickmeister plant höchstens 500 einzelne Klicks pro Sekunde: bei Einzelklicks 500 Zyklen pro Sekunde (2 ms), bei Doppelklicks 250 Zyklen pro Sekunde (4 ms). Wechselst du zum Doppelklick, hebt Klickmeister ein kürzeres Intervall auf 4 ms an. Wie viele Klicks tatsächlich ankommen, hängt bei so kurzen Intervallen von Portal, Compositor und Rechner ab und kann unter der geplanten Rate liegen. Auf einem Intel Core i5-7200U unter KWin 6.7.5 kamen bei fester Position und geplanten 500 Klicks/s etwa 498 Einzelklicks bzw. 500 Klicks als Doppelklicks pro Sekunde an. Gemessen wurde mit dem Benchmark-Testziel, das höchstens alle 100 ms neu zeichnet. Zeichnet das Testziel nach jedem Mausereignis neu, belegt es selbst fast einen CPU-Kern, und es kommen etwa 475 bzw. 500 Klicks pro Sekunde an. Klickmeister sendet die Klicks dafür über libei direkt an KWin; ein Portal ohne diesen Weg (`ConnectToEIS`) erreicht auf demselben Rechner nur etwa 410 bzw. 485 Klicks pro Sekunde. Die Oberfläche zeigt die Zyklus- und Klickrate an; bei Intervallen über einer Sekunde zeigt sie den Abstand zwischen den Zyklen.

## Installation

Klickmeister benötigt KDE Plasma 6, eine Wayland-Sitzung und den KDE-Portaldienst. Das mitgelieferte Paket ist für Arch Linux und darauf basierende Distributionen wie CachyOS vorgesehen.

### Fertiges Paket herunterladen

1. Öffne die [Downloads der neuesten Version](https://github.com/Jerry0205dotde/Auto-Clicker/releases/latest).
2. Lade die Datei **`klickmeister-…-x86_64.pkg.tar.zst`** herunter.
3. Öffne ein Terminal im Downloadordner und installiere die Datei:

```bash
sudo pacman -U ./klickmeister-*-x86_64.pkg.tar.zst
```

Pacman installiert auch die benötigten Laufzeitabhängigkeiten. Danach ist **Klickmeister** im KDE-Anwendungsmenü verfügbar. Git, Rust und das Klonen des Projekts sind dafür nicht nötig. Verwende ein vollständig aktualisiertes Arch-/CachyOS-System mit x86_64-Prozessor.

Die Archive mit **`-aur.tar.gz`** im Namen sind für Paketbetreuer bestimmt. Zum direkten Installieren verwendest du die **`.pkg.tar.zst`**-Datei.

### Über das AUR

Die Releases enthalten fertige Paketbeschreibungen für **`klickmeister-bin`** (ohne Kompilieren) und **`klickmeister`** (aus Quellcode). Sobald ein Betreuer sie im AUR veröffentlicht hat, ist die Installation beispielsweise mit einem vorhandenen AUR-Helfer möglich:

```bash
yay -S klickmeister-bin
```

Das Erzeugen eines Releases veröffentlicht die Paketbeschreibungen noch nicht im AUR. Die Schritte dafür stehen unter [Releases und AUR](docs/RELEASING.md).

## Erste Schritte

1. Öffne Klickmeister und bestätige im KDE-Dialog das Tastenkürzel für Start und Stopp.
2. Wähle Maustaste, Klickart und Klickintervall.
3. Lege die Wiederholungen und die Klickposition fest.
4. Drücke das Tastenkürzel oder wähle **Starten**. Bestätige die Freigabe für die Maussteuerung im KDE-Dialog.
5. Drücke das Tastenkürzel erneut oder wähle **Stoppen**, um das Klicken zu beenden.

Klickmeister läuft pro Sitzung nur einmal. Ein weiterer Start, etwa durch einen Doppelklick im Anwendungsmenü, öffnet kein zweites Fenster, sondern holt das vorhandene in den Vordergrund. So erreicht jeder Druck auf das Tastenkürzel genau einen Klickmeister.

Als Tastenkürzel wird `Pause` vorgeschlagen. Das aktive Kürzel steht in der Fußleiste; mit **Ändern …** lässt es sich im KDE-Dialog anpassen. Ältere Portale ohne diesen Dialog (GlobalShortcuts Version 1) verweisen stattdessen auf die Systemeinstellungen unter „Kurzbefehle“. Klickmeister startet erst, wenn ein Tastenkürzel für Start und Stopp eingerichtet ist.

### Start an der Mauszeigerposition

Bei **Mauszeiger** folgt jeder Klick dem Mauszeiger. Beim Start über die Schaltfläche beginnt nach der Freigabe ein Countdown von drei Sekunden. Bewege den Mauszeiger in dieser Zeit zum Ziel. **Start abbrechen** oder das Tastenkürzel beendet den Countdown ohne Klick.

Beim Start per Tastenkürzel beginnt das Klicken nach der Freigabe sofort. Das gilt auch beim Start an einer festen Position.

### Feste Position wählen

1. Wähle **Fester Punkt** und den gewünschten **Bildschirm**.
2. Wähle **Auswählen …**. Die Positionsauswahl öffnet sich auf diesem Bildschirm.
3. Setze die Position mit einem Linksklick. Mit den Pfeiltasten verschiebst du sie um einen Schritt, mit `Umschalt` + Pfeiltasten um zehn Schritte.
4. Bestätige mit `Enter`. `Esc` oder ein Rechtsklick bricht die Auswahl ab.

Nach einem Linksklick oder einer Korrektur per Pfeiltasten bleibt die Position fixiert; Mausbewegungen verändern sie dann nicht mehr. Das Augensymbol **Punkt anzeigen** markiert das gespeicherte Ziel kurz, ohne zu klicken.

Das Hauptfenster wird während der Auswahl minimiert und danach mit seiner bisherigen Größe, Position und Maximierung wiederhergestellt. Die Auswahl liegt direkt über dem Desktop. Sie erstellt keine Bildschirmaufnahme und verwendet keine Lupe.

Das Hauptfenster kann auf einem anderen Monitor bleiben als die feste Position. Klickmeister ordnet die von KDE gelieferten Monitorinformationen vor dem ersten Klick anhand von Position und logischer Größe eindeutig zu. Fehlende, nicht passende oder mehrdeutige Angaben brechen den Start ab; gespiegelte Ausgänge mit identischer Geometrie werden nicht unterstützt.

Eine feste Position bleibt mit ihrem Bildschirm gespeichert. Ändern sich Auflösung oder Skalierung, muss sie erneut bestätigt werden. Ist der Bildschirm nicht verfügbar oder nicht eindeutig wiederzuerkennen, prüfe Bildschirm und Koordinaten und wähle **Position bestätigen** oder eine neue Position. Die gespeicherte Position bleibt erhalten, bis du sie bestätigst oder ersetzt.

## Freigaben und Datenschutz

KDE fragt nach Freigaben für das globale Tastenkürzel und die Maussteuerung. Bei einer festen Position kommt eine Bildschirmfreigabe für die Monitor-Streams aller Ausgänge hinzu, damit Klickmeister die Koordinaten eindeutig zuordnen kann. Verwendet werden nur die Monitorinformationen; Bild- oder Videodaten werden nicht gelesen.

Nach einem Lauf bleibt die Freigabe für die Maussteuerung noch 60 Sekunden bestehen, bei fester Position zusammen mit der Bildschirmfreigabe. Ein erneuter Start in dieser Zeit kommt ohne KDE-Dialog aus. Danach beendet Klickmeister die Freigaben, und der nächste Start fragt erneut. Solange die Freigabe besteht, zeigt KDE in der Systemleiste das Symbol „Fernsteuerung“. Dessen Aktion **Beenden** hebt die Freigabe sofort auf. Klickmeister zeigt dann „Freigabe erneut erforderlich“, und der nächste Start fragt wieder nach.

Klickmeister liest keine Tastatureingaben, Passwörter oder Zwischenablagen und fordert keine Screenshots an. Gespeichert werden nur die Einstellungen. Die Anwendung erfasst keine Nutzungsdaten oder Telemetrie und benötigt keinen Netzwerkzugriff.

Mit dem Schließen des Fensters endet die Anwendung einschließlich ihrer Freigaben. Es gibt keinen Hintergrunddienst und keinen Autostart.

## Häufige Fragen

### Warum ist „Starten“ nicht verfügbar?

Für jeden Lauf muss das Tastenkürzel für Start und Stopp eingerichtet sein. Falls die Freigabe abgelehnt, die Taste entfernt oder die Freigabe beendet wurde, wähle in der Fußleiste die Schaltfläche **Erneut versuchen**. Ein Neustart ist dafür nicht nötig.

### Warum wird eine feste Position nicht übernommen?

Eine feste Position gilt nur für ihren ausgewählten Bildschirm, unabhängig vom Standort des Hauptfensters. Wird dieser Bildschirm in der Anordnung verschoben, stoppt ein laufender Klicklauf mit fester Position. Die Position bleibt dabei bestätigt. Nach Änderungen an Auflösung oder Skalierung sowie bei einer fehlenden Bildschirmzuordnung muss die Position dagegen in Klickmeister geprüft und erneut bestätigt werden.

### Wann werden die Einstellungen gespeichert?

Die Einstellungen werden beim Start eines Klicklaufs und beim Schließen gespeichert. Änderungen werden auch übernommen, wenn kein Lauf gestartet wurde. Schlägt das Speichern beim Schließen fehl, kannst du zur Anwendung zurückkehren oder **Ohne Speichern schließen** wählen. Ein laufender Klicklauf wird dabei gestoppt; das Tastenkürzel bleibt eingerichtet, und es gibt keine erneute Freigabeabfrage.

### Warum öffnet sich kein zweites Fenster?

Klickmeister läuft pro Sitzung nur einmal. KDE leitet einen Druck auf das Tastenkürzel an jede laufende Instanz weiter; zwei Instanzen würden sich gegenseitig starten und stoppen. Ein weiterer Start zeigt deshalb das vorhandene Fenster an, während einer Positionsauswahl die Auswahl. Wird Klickmeister gerade geschlossen, wartet ein neuer Start bis zu zehn Sekunden auf dessen Ende und öffnet dann ein eigenes Fenster.

### Funktioniert Klickmeister unter X11?

Klickmeister ist für KDE Plasma unter Wayland ausgelegt. X11 wird nicht unterstützt. Wie Klickmeister auf X11, anderen Desktops, Windows und macOS laufen könnte, beschreibt der Plan unter [Klickmeister auf weiteren Systemen](docs/CROSS_PLATFORM.md).

## Entwicklung

Wie du Klickmeister aus dem Quellcode baust, prüfst und einen lokalen Build beim Portal anmeldest, steht in [CONTRIBUTING.md](CONTRIBUTING.md). Aufbau und Sicherheitsentscheidungen beschreiben [ARCHITECTURE.md](ARCHITECTURE.md) und [SECURITY_REVIEW.md](SECURITY_REVIEW.md).

## Lizenz

Klickmeister ist freie Software unter der [MIT-Lizenz](LICENSE).
