# Klickmeister auf weiteren Systemen

Dieses Dokument beschreibt, wie Klickmeister über KDE Plasma unter Wayland hinaus auf anderen Desktops und Betriebssystemen laufen kann. Es ist ein Plan; nichts davon ist umgesetzt. Das heutige Verhalten unter Plasma bleibt die Referenz, an der sich jedes weitere Backend messen muss.

## Ziel und Grenzen

| System | Ziel | Bemerkung |
| --- | --- | --- |
| KDE Plasma 6, Wayland | wie heute | Referenzplattform |
| GNOME 48 und neuer, Wayland | voller Funktionsumfang | dieselben Portale wie unter KDE |
| GNOME vor 48 | eingeschränkt | ohne GlobalShortcuts-Portal: Kürzel nur über eine Desktop-Tastenkombination, die `klickmeister --toggle` aufruft |
| Linux unter X11 (alle Desktops) | voller Funktionsumfang | eigenes X11-Backend ohne Portal |
| Sway, Hyprland, andere wlroots-Compositoren | eingeschränkt | Klicks über `wlr-virtual-pointer`, Kürzel per Compositor-Bindung |
| Windows 10/11 | voller Funktionsumfang | natives Backend |
| macOS 13 und neuer | voller Funktionsumfang | natives Backend, Bedienungshilfen-Freigabe nötig |
| Android, iOS | nicht vorgesehen | siehe unten |

„Eingeschränkt“ steht nur dort, wo das System selbst die nötige Schnittstelle nicht anbietet. Was Klickmeister selbst beitragen kann, muss dort ebenfalls dem [Qualitätsmaßstab](#qualitätsmaßstab) genügen.

## Qualitätsmaßstab

Jede Plattform muss mindestens so gut sein wie Klickmeister heute unter Plasma, und sie soll sich wie eine native Anwendung des jeweiligen Systems anfühlen. Ein Backend gilt erst als fertig, wenn es alle folgenden Punkte erfüllt:

- **Geschwindigkeit:** Der native Benchmark (`tests/native/benchmark.py` oder ein gleichwertiges Testziel auf der Plattform) erreicht bei fester Position und geplanten 500 Klicks/s mindestens die heutigen Plasma-Werte, also etwa 365 Einzelklicks bzw. 440 Klicks als Doppelklicks pro Sekunde, gemessen auf vergleichbarer Hardware. Bei Raten, die Plasma heute vollständig zustellt, geht auch dort kein Klick verloren, und Druck und Loslassen kommen immer paarweise an. Unter Plasma darf kein Umbau die gemessenen Werte verschlechtern; jeder Backend-PR enthält eine Messung vorher und nachher.
- **Funktionsumfang:** Alle Punkte aus [Funktionen](../README.md#funktionen) und [Erste Schritte](../README.md#erste-schritte) funktionieren: alle Maustasten, Einzel- und Doppelklick, Mauszeiger und feste Position auf jedem Monitor, Positionsauswahl mit Pfeiltasten, Countdown, Wiederholungen, globales Kürzel für Start und Stopp, Einzelinstanz.
- **Sicherheit und Transparenz:** Solange Klickmeister klicken darf, ist das sichtbar und mit einem Griff zu beenden, wie das KDE-Symbol „Fernsteuerung“. Wo das System selbst nichts anzeigt (Windows, X11, macOS), übernimmt das ein eigenes Symbol in der Taskleiste bzw. Menüleiste mit **Stoppen**. Klickmeister liest weiterhin keine Tastatureingaben außer dem eigenen Kürzel, keine Bildschirminhalte und sendet keine Daten.
- **Nativ:** Aussehen, Dialoge, Tastenkürzel-Eingabe, Berechtigungsabfragen, Speicherorte, Installation und Updates folgen den Konventionen der Plattform (siehe [Oberfläche](#oberfläche) und [Pakete und CI](#pakete-und-ci)). Hell/Dunkel-Modus, Akzentfarbe, Skalierung und Bildschirmleser funktionieren wie bei anderen Anwendungen des Systems.
- **Geprüft auf echter Hardware:** Neben der CI läuft auf jeder Plattform die Prüfliste aus [tests/native](../tests/native/README.md) sinngemäß auf einem echten Gerät, einschließlich mehrerer Monitore mit unterschiedlicher Skalierung.

Wo die Plattform mehr erlaubt als das Portal, nutzt Klickmeister das. Windows, X11 und macOS erzeugen Klicks ohne D-Bus-Umweg und sollten die 500 Klicks/s deutlich näher erreichen. Unter Linux soll libei (siehe unten) auch Plasma selbst schneller machen.

## Nicht vorgesehen

Auf **Android** ginge es nur über einen `AccessibilityService` mit `dispatchGesture`: eine eigene Anwendung mit anderer Oberfläche, eigenem Berechtigungsmodell und strengen Play-Store-Regeln für solche Dienste. Auf **iOS** dürfen Apps grundsätzlich keine Eingaben in andere Apps erzeugen. Beides ist deshalb kein Ziel dieses Plans.

Ein Wayland-Compositor, der weder RemoteDesktop noch `wlr-virtual-pointer` anbietet, lässt sich ohne seine Mitwirkung nicht steuern. Einen Weg über `/dev/uinput` schließt Klickmeister weiterhin aus: Er bräuchte Root-Rechte oder eine udev-Regel und umginge jede Freigabe durch den Desktop.

## Was heute plattformgebunden ist

Portal-spezifisch heißt nicht KDE-spezifisch. RemoteDesktop, ScreenCast, GlobalShortcuts und xdg-foreign sind desktopübergreifende Schnittstellen. KDE-gebunden sind vor allem einige Annahmen, Texte, der Stil und die bisherige Prüfung.

| Bereich | Stelle | Bindung |
| --- | --- | --- |
| Klick-Eingabe | `src/portal.rs` (`PortalClickSession`) | RemoteDesktop mit `NotifyPointerMotionAbsolute`/`NotifyPointerButton` |
| Maustastencodes | `src/model.rs` (`MouseButton::evdev_code`) | Linux-evdev-Codes im sonst neutralen Modell |
| Feste Position | `src/portal.rs` (`select_monitor_stream`) | ScreenCast-Streams mit Position und Größe; diese Metadaten sind laut Spezifikation optional |
| Globales Kürzel | `src/worker.rs` (`setup_hotkey`, `HotkeyState`, `StartBarrier`) | GlobalShortcuts-Portal direkt im Worker, Schutz vor verspäteten Tastendrücken über die D-Bus-Nachrichtenreihenfolge |
| Startsperre | `src/worker.rs`, `qml/Main.qml` | ohne registriertes Kürzel kein Start, auch wenn Klicks möglich wären |
| Elternfenster | `src/parent_window.rs`, `src/qml_runtime.cpp` | `KWaylandExtras::exportToplevel` (ab KDE Frameworks 6.28) |
| Einzelinstanz | `src/instance.rs`, `src/main.rs` | Session-D-Bus, `org.freedesktop.Application.Activate`, `XDG_ACTIVATION_TOKEN` |
| Einstellungen | `src/config.rs` | nur `XDG_CONFIG_HOME` bzw. `~/.config` |
| Oberfläche | `src/main.rs`, `qml/*.qml` | fester Stil `org.kde.desktop`, Kirigami, Symbole aus dem Desktop-Thema |
| Build und Pakete | `Cargo.toml`, `build.rs`, `PKGBUILD` | `ashpd` und `KF6WindowSystem` immer eingebunden, Paket verlangt `xdg-desktop-portal-kde` |

Plattformneutral sind schon heute `src/state.rs`, `src/scheduler.rs`, die Einstellungen und Prüfungen in `src/model.rs` sowie der größte Teil von `src/controller.rs` und `src/worker.rs`. Die Regeln aus [ARCHITECTURE.md](../ARCHITECTURE.md) für Stop, Abbruch, Startreservierung und Lebensdauer gelten unverändert für alle Backends.

## Aufbau

Statt eines einzigen Plattform-Backends gibt es mehrere kleine Schnittstellen. Sie lassen sich unabhängig kombinieren; unter Hyprland kann das Kürzel etwa über ein Portal kommen, während die Klicks über ein Wayland-Protokoll laufen.

| Modul | Aufgabe |
| --- | --- |
| `backend/input` | `InputBackend::prepare(ziel, abbruch)` fordert die Freigabe an und liefert eine `ClickSession` mit `click_cycle()`, Sitzungsende, Wiederverwendbarkeitsprüfung und begrenztem Schließen |
| `backend/shortcuts` | Kürzel registrieren, ändern, Ereignisse `Activated`, `BindingChanged`, `Unavailable`; jedes Backend schützt selbst vor verspäteten Tastendrücken |
| `platform/displays` | Monitore, logische Geometrie und Übersetzung in den Koordinatenraum des Backends |
| `platform/instance` | Einzelinstanz beanspruchen, laufende Instanz aktivieren |
| `platform/paths` | Speicherort der Einstellungen |
| `platform/window` | Elternfenster für Freigabedialoge und Fensteraktivierung |

Die Implementierungen liegen in `backend/portal`, `backend/x11`, `backend/wlr`, `backend/windows` und `backend/macos`. Plugins zur Laufzeit sind nicht nötig; Rust-Module mit `cfg(target_os = …)` und zielabhängigen Abhängigkeiten in `Cargo.toml` reichen. Unter Linux wählt Klickmeister beim Start anhand von `WAYLAND_DISPLAY`/`DISPLAY` und der vorhandenen Portal-Schnittstellen.

Dabei gilt:

- **Fähigkeiten und Freigaben bleiben getrennt.** „Feste Position möglich“, „Kürzel nur extern einstellbar“ und „Maussteuerung noch nicht freigegeben“ sind verschiedene Zustände. Der Controller erhält sie als `InputCapabilities`, `ShortcutCapabilities` und Freigabestatus, und die Oberfläche blendet nicht verfügbare Optionen mit Begründung aus.
- **Die Startsperre hängt an der Fähigkeit, nicht am Portal.** Ohne globales Kürzel bleiben die Knöpfe **Starten** und **Stoppen** sowie `klickmeister --toggle` nutzbar. Der Toggle-Aufruf geht über die Einzelinstanz an das laufende Fenster.
- **Kürzel werden maschinenlesbar gespeichert.** `AppConfig.hotkey` enthält heute die vom Portal gelieferte Beschreibung. Native Backends brauchen eine Kombination wie `Pause` oder `Ctrl+F8`; die Anzeige bleibt davon getrennt. `Pause` fehlt auf vielen Laptop- und Mac-Tastaturen, daher wählt jede Plattform ihren eigenen Vorschlag.
- **Native Ereignisse kommen über Kanäle in den Worker.** Zustandsautomat, Countdown, Scheduler und priorisierter Stop bleiben dort. Hotkeys unter macOS müssen jedoch auf dem Hauptthread registriert werden, unter Windows braucht `RegisterHotKey` eine Nachrichtenschleife. Diese Backends laufen deshalb auf dem Qt-Thread bzw. einem eigenen Thread und melden nur Ereignisse.
- **Koordinaten bleiben monitorlokal und logisch.** Erst das Backend übersetzt sie: in Stream-Koordinaten (Portal), Root-Fenster-Koordinaten (X11), den Raum des gewählten `wl_output` (wlr), physische Pixel des Monitors und normalisierte Koordinaten mit `MOUSEEVENTF_VIRTUALDESK` (Windows) oder Quartz-Punkte (macOS). Ein globaler Skalierungsfaktor reicht bei gemischt skalierten Monitoren nicht.

## Backends

| Plattform | Klicks | Globales Kürzel | Einschränkungen |
| --- | --- | --- | --- |
| Plasma, Wayland | bestehendes RemoteDesktop-Backend | bestehendes GlobalShortcuts-Portal | keine |
| GNOME, Wayland | dasselbe Portal-Backend | GlobalShortcuts ab GNOME 48 | Monitor-Metadaten der Streams prüfen; fehlen sie, nur Mauszeigermodus |
| X11 | `x11rb` mit XTEST | `XGrabKey` über `x11rb` | gilt nur für echte X11-Sitzungen, nicht für Wayland-Fenster über Xwayland |
| Sway, wlroots | `wlr-virtual-pointer` v2 über `wayland-client` | Compositor-Bindung an `klickmeister --toggle` | `xdg-desktop-portal-wlr` bietet weder RemoteDesktop noch GlobalShortcuts |
| Hyprland | `wlr-virtual-pointer` | GlobalShortcuts-Portal von `xdg-desktop-portal-hyprland` | kein RemoteDesktop; InputCapture ersetzt keine Eingabe |
| Windows | `SendInput` über das Crate `windows` | `RegisterHotKey` mit `MOD_NOREPEAT` | keine Eingaben in Fenster mit höherer Integritätsstufe (Admin-Programme) |
| macOS | `CGEventPost` über `objc2-core-graphics`, Doppelklick mit `kCGMouseEventClickState` | `RegisterEventHotKey` | Freigabe unter **Bedienungshilfen** nötig und an die Signatur gebunden |

**Portal und libei.** RemoteDesktop bietet mit `ConnectToEIS` einen Eingabekanal über libei (in Rust: `reis`). Klicks laufen dann nicht mehr als einzelne D-Bus-Aufrufe. Das kann den unter KWin 6.7.5 beobachteten Verlust von Klicks bei Druck und Loslassen ohne Zwischenantwort beheben und die erreichbare Klickrate erhöhen; beides ist zu messen. Nach dem Wechsel auf EIS darf dieselbe Sitzung keine `Notify*`-Aufrufe mehr senden.

**Feste Position ohne Metadaten.** Liefert ein Portal keine Position und Größe pro Stream, darf Klickmeister den Monitor nicht aus Reihenfolge oder Anzahl der Streams raten. Dann bleibt nur der Mauszeigermodus, und die Oberfläche sagt das.

**`enigo`** deckt Klicks unter Windows, macOS und X11 ab und kann hinter `InputBackend` für einen schnellen Prototyp dienen. Kürzel, Monitorzuordnung und Fensterintegration löst es nicht, seine Wayland-Unterstützung ist experimentell. Die Backends sind dünn genug, um sie selbst zu schreiben.

## Oberfläche

Qt/QML bleibt. Qt zeichnet unter Windows und macOS mit den nativen Stilen des Systems, ein Wechsel des Toolkits löst keines der Eingabe- oder Freigabeprobleme und würde QML-Tests, Barrierefreiheit und die Positionsauswahl neu erfordern. Für ein natives Aussehen hängt die Oberfläche aber nicht mehr überall an Kirigami:

| Plattform | Stil | Besonderheiten |
| --- | --- | --- |
| Plasma | `org.kde.desktop` mit Kirigami, wie heute | unverändert |
| GNOME | Fusion mit Systempalette, Hell/Dunkel und Akzentfarbe über das Settings-Portal | Dialoge über die Portale, also im GNOME-Stil; `QGnomePlatform` und `adwaita-qt` werden nicht mehr gepflegt |
| Windows 11 | `FluentWinUI3` (ab Qt 6.8), unter Windows 10 `Windows` | Mica/Akzentfarbe, Taskleisten-Symbol, Benachrichtigungen |
| macOS | `macOS`-Stil von Qt | Menüleiste mit Einstellungen und Beenden (`Cmd+Q`), Menüleisten-Symbol |

- Kirigami wird nur noch für `Units` und `Theme` verwendet. Dafür gibt es ein kleines eigenes QML-Singleton, das unter Plasma Kirigami-Werte und sonst `SystemPalette` und Schriftmetriken liefert. Damit entfällt Kirigami auf Windows und macOS, und die Oberfläche übernimmt Farben, Abstände und Schrift des Systems.
- `main.rs` wählt den Stil passend zur Plattform; `org.kde.desktop` nur, wenn `qqc2-desktop-style` vorhanden ist.
- `KWindowSystem` wird nur unter Linux gebaut und gelinkt, `build.rs` entscheidet anhand von `CARGO_CFG_TARGET_OS`. Der Fensterexport braucht eine Versionsprüfung, weil `exportToplevel` erst ab KDE Frameworks 6.28 existiert.
- Unter Plasma und GNOME stellt der Desktop den Dialog für das Tastenkürzel. Auf Windows, macOS, X11 und wlroots bietet Klickmeister einen eigenen Eingabedialog im Stil der Plattform, der Konflikte mit belegten Kombinationen sofort meldet.
- Symbole kommen unter Linux weiterhin aus dem Desktop-Thema, auf Windows und macOS aus mitgelieferten Ressourcen im Stil der jeweiligen Plattform.
- Texte nennen das jeweilige System statt pauschal „KDE“, etwa „Bedienungshilfen“ unter macOS.
- `PositionPicker.qml` bleibt ein transparentes Vollbildfenster je Monitor und fordert weiterhin keine Bildschirmaufnahme an. Vollbild, Fokus und Transparenz sind auf macOS (Spaces) und unter Wayland-Compositoren ohne KDE einzeln zu prüfen.
- Einstellungen liegen unter Windows in `%APPDATA%\Klickmeister`, unter macOS in `~/Library/Application Support/Klickmeister`, unter Linux unverändert in `~/.config`.

## Einzelinstanz

Unter Linux bleibt der D-Bus-Name. Unter Windows und macOS übernimmt ein gemeinsamer Qt-Helfer aus `QLockFile` und `QLocalServer` mit einem benutzerbezogenen Namen dieselbe Aufgabe, einschließlich Aktivierung und `--toggle`. Die Sperre muss wie heute vor jeder Kürzel-Registrierung bestehen. Mit einem X11-Backend funktionieren Klicks und Kürzel auch ohne Session-Bus; der heutige Start ohne Sperre in diesem Fall muss dann entfallen oder auf die Sperrdatei ausweichen.

## Pakete und CI

| Ziel | Vorschlag |
| --- | --- |
| Flatpak (alle Distributionen) | KDE-Runtime, die Kirigami und `qqc2-desktop-style` schon enthält; Cargo-Abhängigkeiten über `flatpak-cargo-generator`; Portale funktionieren in der Sandbox unverändert; Veröffentlichung auf Flathub |
| AppImage | optional; bündelt Qt, Kirigami und QML-Module, setzt aber Portale auf dem Wirt voraus |
| Windows | MSVC-Build mit Qt aus `install-qt-action`, `windeployqt --qmldir qml`; signiertes MSIX bzw. Installer, verteilt über `winget` und optional den Microsoft Store, damit Updates wie bei anderen Programmen laufen |
| macOS | Universal-`.app` (arm64 und x86_64), `macdeployqt`, signiertes und notarisiertes `.dmg`, zusätzlich Homebrew-Cask; ohne Apple-Developer-Konto warnt Gatekeeper, und die Bedienungshilfen-Freigabe geht bei jeder unsignierten neuen Version verloren, deshalb ist die Signatur für „nativ“ Pflicht |
| Arch/AUR | wie heute; `xdg-desktop-portal-kde` wird zur optionalen Abhängigkeit neben `xdg-desktop-portal-gnome` |

In der CI kommen Matrix-Jobs für `windows-latest` und `macos-latest` hinzu (Build, Rust-Tests, QML-Smoke-Test), ein X11-Job unter Xvfb mit XTEST-Prüfung und ein Job mit headless Sway für das wlr-Backend. Auf dem Windows-Runner läuft zusätzlich der Benchmark gegen ein Testfenster, da GitHub dort eine echte Desktop-Sitzung bietet. Unter macOS lässt sich die Bedienungshilfen-Freigabe in der CI nicht erteilen; dort bleibt der Benchmark eine Prüfung auf einem echten Mac. Die bestehenden Arch-Jobs, die Portaltests auf privatem D-Bus und der virtuelle KWin bleiben. Echte Freigabedialoge und gemischt skalierte Monitore bleiben manuelle Prüfungen auf dem jeweiligen System, wie heute unter [tests/native](../tests/native/README.md).

## Reihenfolge

Jede Phase ist ein eigener PR oder eine kleine Reihe davon. Nach jeder Phase muss sich Klickmeister unter Plasma genau wie vorher verhalten.

| Phase | Ergebnis | Aufwand (Personentage) |
| --- | --- | ---: |
| 1 | `InputBackend` einführen, Portal-Klicks dahinter, `evdev_code` ins Portal-Backend | 2–3 |
| 2 | `ShortcutBackend` einführen, Portal-Kürzel samt Startbarriere dahinter, Startsperre an Fähigkeiten, `--toggle` | 3–5 |
| 3 | portabler Build: eigenes Theme-Singleton statt Kirigami, Stilwahl, Symbole, Pfade, `build.rs` nach Zielsystem, Einzelinstanz-Abstraktion | 5–8 |
| 4 | GNOME prüfen und Texte von „KDE“ lösen, Flatpak | 4–6 |
| 5 | X11-Backend | 3–5 |
| 6 | Windows-Backend, Taskleisten-Symbol, Kürzel-Dialog, signiertes Paket | 8–12 |
| 7 | macOS-Backend, Menüleiste, Bedienungshilfen-Abfrage, signiertes Paket | 9–13 |
| 8 | wlroots/Hyprland über `wlr-virtual-pointer` | 5–8 |
| 9 | libei im Portal-Backend für mehr Geschwindigkeit unter Linux | 4–7 |

Insgesamt etwa 48–74 Personentage. libei ist keine Option mehr, sondern der Weg, auch unter Linux über die heutigen Werte hinauszukommen. Flatpak in Phase 4 bringt mit wenig Aufwand die meisten zusätzlichen Linux-Nutzer; Windows bringt die meisten Nutzer überhaupt, braucht aber die Vorarbeit aus den Phasen 1 bis 3.

Der erste PR ist eine reine Verschiebung ohne Verhaltensänderung: `backend/input` mit genau den Operationen, die der Worker heute an `PortalClickSession` aufruft (`create`, `click`, `matches_monitor`, `is_closed`, `wait_closed`, `close`). Die vorhandenen Portaltests bleiben, dazu kommen Vertragstests mit einem simulierten Backend für Abbruch und Sitzungsverlust. Countdown, Stop, die 60 Sekunden bis zum Schließen der Freigabe und alle Fehlermeldungen bleiben gleich, und der Benchmark auf dem Server zeigt vorher und nachher dieselben Klickraten. Die Kürzel-Extraktion folgt getrennt, weil sie die empfindliche Startbarriere betrifft.
