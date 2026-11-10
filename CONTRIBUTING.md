# Mitwirken

Diese Seite beschreibt, wie du Klickmeister aus dem Quellcode baust und prüfst. Wie die Anwendung benutzt wird, steht in der [README](README.md).

## Build-Abhängigkeiten

Installiere auf Arch Linux die Build- und Laufzeitabhängigkeiten:

```bash
sudo pacman -S --needed base-devel git rust cargo clang lld pkgconf qt6-base qt6-declarative qt6-wayland qt6-tools kirigami kwindowsystem qqc2-desktop-style xdg-desktop-portal xdg-desktop-portal-kde python
```

Cargo-Befehle bauen den aktuellen Checkout einschließlich lokaler Änderungen. Der `PKGBUILD` im Projektstamm verwendet einen festgelegten, mit Prüfsumme gesicherten Quellstand für Version 0.1.4. Mit `makepkg --allsource --nodeps` lässt sich ein Quellpaket einschließlich des Projektarchivs erstellen und in einem anderen Verzeichnis bauen. Cargo-Abhängigkeiten werden beim Paketbau mit `cargo fetch --locked` geladen.

`bash scripts/build-release.sh` baut den aktuellen Checkout als installierbares Paket und erzeugt die AUR-Paketbeschreibungen samt Prüfsummen in `dist/`. Der GitHub-Workflow **Release packages** führt das bei einem Versionstag automatisch aus, prüft die Installation und stellt die Dateien unter [Releases](https://github.com/Jerry0205dotde/Auto-Clicker/releases) bereit. Details stehen unter [Releases und AUR](docs/RELEASING.md).

## Bauen und prüfen

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo deny check advisories bans licenses sources # cargo-deny, Regeln in deny.toml
dbus-run-session -- cargo test --locked --all-targets -- --include-ignored
cargo build --locked --release
bash tests/qml-smoke.sh target/release/klickmeister
QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software QT_QUICK_CONTROLS_STYLE=org.kde.desktop /usr/lib/qt6/bin/qmltestrunner -import tests/qml/mocks -input tests/qml
python tests/check_coordinate_accessibility.py # grafische Sitzung mit AT-SPI und PyGObject
bash tests/qml-wayland.sh # isolierter KWin mit drei Bildschirmen, benötigt kscreen-doctor
KLICKMEISTER_TEST_SCALE=1.5 bash tests/qml-wayland.sh
```

Die QML-Tests verwenden den KDE-Control-Stil und einen Controller-Testersatz. Die AT-SPI-Prüfung startet das Hauptfenster ohne Worker und prüft die zugänglichen Namen der Koordinatenfelder und ihrer Texteingaben. Die D-Bus-Tests prüfen den Rust-Worker gegen simulierte Portale. Dazu gehören Klickfolgen, Stopps, verzögerte Tastenkürzel-Signale, Sitzungsende und Fehler beim Loslassen einer Maustaste. Ein weiterer D-Bus-Test startet das gebaute Programm ohne Bildschirm zweimal. Der zweite Start muss das erste Fenster anzeigen und sich beenden, ohne eine weitere Tastenkürzel-Sitzung anzulegen; nach einem Absturz der ersten Instanz muss ein neuer Start wieder möglich sein. Diese Tests erzeugen keine Mausklicks auf dem Desktop.

Tastenkürzel-Signale, die sich während eines Laufs angestaut haben, dürfen diesen noch stoppen, danach aber keinen neuen Lauf starten. Eine neue Betätigung nach dem Stopp startet wie gewohnt. Dasselbe gilt für angestaute Aufrufe von `klickmeister --toggle`; ohne Portal-Kürzel startet ein Toggle erst nach der Kürzelprüfung, und der prüfende Toggle klickt nicht.

Echte KDE-Dialoge und die Zeigersteuerung auf physischen Bildschirmen werden zusätzlich in einer nativen Plasma-Wayland-Sitzung geprüft. Die Werkzeuge und Voraussetzungen dafür sind unter [tests/native](tests/native/README.md) dokumentiert; das gebaute Programm braucht dafür die [Desktop-Integration](#desktop-integration).

Prüfe den Start-Countdown manuell an einem eigenen Testziel: Vor Ablauf der drei Sekunden darf kein Klick erfolgen. **Start abbrechen** und das Tastenkürzel müssen den Countdown ohne Klick beenden. Wiederhole die Prüfung mit bereits erteilter Freigabe. Starts per Tastenkürzel und mit fester Position müssen ohne Countdown beginnen.

GitHub Actions führt Formatierung, Clippy, Rust- und Portaltests, QML-Tests im KDE-Stil, die Vollständigkeitsprüfung der deutschen Übersetzung, Python-Tests sowie Desktop- und AppStream-Validierung aus. Danach folgen Release-Build und QML-Smoke-Test. Ein separater Job prüft die QML-Oberfläche unter einem virtuellen KWin mit drei Bildschirmen. Beide Jobs verwenden Arch-Linux-Container und die Cargo-Lockdatei; native Tests mit echten KDE-Dialogen bleiben eine manuelle Prüfung.

Der Workflow „Dependencies“ prüft die Abhängigkeiten mit cargo-deny auf RustSec-Meldungen, Lizenzen und Paketquellen. Er läuft bei Pull Requests und zusätzlich jeden Montag, damit neue Meldungen auch ohne Code-Änderung auffallen. Dependabot schlägt wöchentlich gruppierte Updates für Cargo-Abhängigkeiten und die auf Commit-SHAs festgelegten GitHub Actions vor.

Der virtuelle KWin-Job nutzt die aktuellen Arch-Pakete. Seit KWin 6.8 braucht der virtuelle Compositor ein Render-Gerät; der Container erhält dafür `/dev/udmabuf`, eine GPU ist nicht nötig. Hintergrund, isolierte Reproduktion und Aktualisierung sind unter [tests/virtual-wayland.md](tests/virtual-wayland.md) beschrieben.

## Übersetzungen

Die Texte der Oberfläche sind englisch, die deutsche Übersetzung liegt in `translations/klickmeister_de.ts`. `build.rs` übersetzt sie mit `lrelease` und bettet sie ein; Klickmeister lädt sie passend zur Systemsprache, sonst bleibt die Oberfläche englisch. QML-Texte stehen in `qsTr`, die Texte aus dem Rust-Code ausschließlich in `src/messages.rs` als `translate("Klickmeister", "…")`. Nach dem Ändern eines Textes übernimmt `bash scripts/update-translations.sh` ihn in die `.ts`-Datei; die neue Übersetzung trägst du dort oder mit Qt Linguist ein. `bash scripts/update-translations.sh --check` prüft wie die CI, dass die Datei aktuell und vollständig übersetzt ist. `tests/translation.rs` prüft die deutschen Texte aus dem Rust-Code.

## QML-Sprachserver

Führe zuerst `cargo build --locked` und danach `bash scripts/setup-qmlls.sh` aus. Das Skript ermittelt das aktive Cargo-Buildverzeichnis, auch bei `CARGO_TARGET_DIR` oder `build.target-dir`, und schreibt die von Git ignorierte `.qmlls.ini`. Nach einem Wechsel des Buildverzeichnisses muss das Skript erneut ausgeführt werden. Python 3 ist erforderlich.

`python tests/check_qmlls_checkout.py` prüft den committeten Stand in einem frischen Checkout mit anderem Pfad und fragt die `AppController`-Eigenschaften direkt beim QML-Sprachserver ab.

## Desktop-Integration

Der Qt-Desktop-Dateiname lautet `io.github.jerry0205.klickmeister` und entspricht der installierten `.desktop`-Datei. Der QML-Smoke-Test prüft diesen Wert. Klickmeister meldet auch seine separaten D-Bus-Verbindungen vor dem ersten Portalaufruf mit dieser ID bei der [Portal-Registry](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.host.portal.Registry.html) an. Damit funktioniert die Zuordnung auch beim direkten Start der Binärdatei. Bei älteren Portaldiensten ohne Registry erfolgt die Zuordnung weiterhin über den Startkontext. Unter derselben ID belegt Klickmeister den D-Bus-Namen `io.github.jerry0205.klickmeister` als Sperre gegen eine zweite Instanz und bietet dort `org.freedesktop.Application.Activate` an. Der Bus gibt den Namen mit der Verbindung frei, auch nach einem Absturz.

Das Portal nimmt die Anmeldung nur an, wenn GLib die `.desktop`-Datei in einem Anwendungsverzeichnis findet und ihr `Exec`-Programm auflösen kann. Maßgeblich ist der `PATH` des Portaldienstes, nicht der der Shell; ein Verzeichnis wie `~/.local/bin`, das nur die Shell-Konfiguration ergänzt, genügt deshalb meist nicht. Sonst lehnt das Portal mit `App info not found` ab, das Tastenkürzel bleibt nicht verfügbar und Starten gesperrt. Ein installiertes Paket erfüllt beides mit `/usr/bin/klickmeister`. Für ein gebautes Programm außerhalb des `PATH` eine der beiden Varianten einrichten. Entweder den Build in den `PATH` des Portals verlinken und die unveränderte Datei installieren:

```bash
sudo ln -sf "$PWD/target/release/klickmeister" /usr/local/bin/klickmeister
install -Dm644 packaging/io.github.jerry0205.klickmeister.desktop -t ~/.local/share/applications/
```

Oder eine lokale Datei mit absolutem `Exec` verwenden. Der Pfad steht dort in Anführungszeichen, damit auch Leerzeichen, `&` oder `|` im Checkout-Pfad funktionieren; `\`, `"`, `` ` `` und `$` maskiert `sed` nach der Desktop-Entry-Spezifikation:

```bash
mkdir -p ~/.local/share/applications
bin=$(printf '%s\n' "$PWD/target/release/klickmeister" | sed 's/\\/\\\\\\\\/g; s/["`$]/\\\\&/g')
{ grep -v '^Exec=' packaging/io.github.jerry0205.klickmeister.desktop; printf 'Exec="%s"\n' "$bin"; } > ~/.local/share/applications/io.github.jerry0205.klickmeister.desktop
```

Die lokale Datei überdeckt die eines installierten Pakets; nach der Prüfung Link und Datei wieder entfernen. Läuft Klickmeister schon, versucht **Erneut versuchen** die Anmeldung danach noch einmal.

Details zum Aufbau und zu Sicherheitsentscheidungen stehen in [ARCHITECTURE.md](ARCHITECTURE.md) und [SECURITY_REVIEW.md](SECURITY_REVIEW.md).
