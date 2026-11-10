# Releases und AUR

## Dateien eines Releases

| Datei | Verwendung |
| --- | --- |
| `klickmeister-VERSION-PAKETVERSION-x86_64.pkg.tar.zst` | Fertige Anwendung; mit `sudo pacman -U ./DATEINAME` installieren. |
| `klickmeister-VERSION.tar.gz` | Der beim Paketbau verwendete Quellcode einschließlich Cargo-Lockdatei. |
| `klickmeister-bin-VERSION-PAKETVERSION-aur.tar.gz` | `PKGBUILD` und `.SRCINFO` für das AUR-Paket ohne Kompilieren. |
| `klickmeister-VERSION-PAKETVERSION-aur.tar.gz` | `PKGBUILD` und `.SRCINFO` für das AUR-Paket aus Quellcode. |
| `SHA256SUMS` | Prüfsummen der vier Download-Dateien. |

Download-Dateien gemeinsam in einem Ordner mit `sha256sum --check SHA256SUMS` prüfen. Die AUR-Beschreibungen enthalten zusätzlich BLAKE2-Prüfsummen der jeweiligen Release-Datei. Es gibt keine ungeprüften `SKIP`-Quellen.

## Release bauen und veröffentlichen

Der Workflow `.github/workflows/release.yml` startet beim Push eines Tags wie `v0.1.2`. Die Version muss zu `Cargo.toml` und `PKGBUILD` passen. Bei neuen Anwendungsversionen auch `Cargo.lock` und die AppStream-Versionsinformationen aktualisieren. `pkgrel` in `PKGBUILD` und `.SRCINFO` gemeinsam pflegen und bei einer neuen Anwendungsversion auf `1` zurücksetzen.

Der Workflow baut auf Arch Linux als normaler Benutzer. Er erstellt ein Quellarchiv des Checkouts und verwendet es für `makepkg`, statt den älteren, im Projektstamm festgelegten Quellstand zu bauen. Vor der Veröffentlichung laufen Rust-Tests und der QML-Smoke-Test im `check()` des Pakets. Außerdem wird die binäre AUR-Beschreibung mit der echten Paketdatei gebaut. Danach installiert der Workflow das Arch-Paket und prüft den Start der installierten Anwendung. Erst nach erfolgreichen Prüfungen legt der zweite Job ein GitHub-Release mit den Downloads an.

**Release packages → Run workflow** baut dieselben Dateien zum Ausprobieren als Actions-Artefakt. Ein manueller Lauf veröffentlicht kein Release. Bereits veröffentlichte Versionen bleiben unverändert; verwende für die nächste Veröffentlichung einen neuen Versionstag.

Lokal auf einem aktuellen Arch-/CachyOS-System mit den [Build-Abhängigkeiten](../CONTRIBUTING.md#build-abhängigkeiten):

```bash
bash scripts/build-release.sh
```

Das Skript muss als normaler Benutzer laufen. Es verwendet den aktuellen Checkout einschließlich lokaler Quelldateiänderungen und installiert keine Build-Abhängigkeiten. Für eine Veröffentlichung einen geprüften, committeten Stand verwenden. Alle Ergebnisse stehen in `dist/`; die entpackten AUR-Beschreibungen stehen zusätzlich in `dist/aur/`.

## Im AUR veröffentlichen

Das AUR erhält nur die Paketbeschreibung. Die fertigen Binärdateien und der Quellcode bleiben bei GitHub Releases. Für das Einreichen ist ein AUR-Konto mit eingerichtetem SSH-Schlüssel erforderlich. Beide Paketnamen vorab auf vorhandene Einträge prüfen; bestehende Pakete nur mit entsprechender Berechtigung aktualisieren.

1. Zuerst das GitHub-Release veröffentlichen, damit die Download-Adressen erreichbar sind.
2. `klickmeister-bin-VERSION-PAKETVERSION-aur.tar.gz` herunterladen und entpacken.
3. Im `PKGBUILD` den Maintainer-Kommentar durch den eigenen Namen und eine Kontaktadresse ersetzen. `makepkg --printsrcinfo > .SRCINFO` ausführen, falls Paketmetadaten geändert wurden.
4. Die Paketbeschreibung in einem eigenen Ordner mit `makepkg -si` prüfen. Sie lädt das fertige Paket und kompiliert keine Rust- oder Qt-Dateien.
5. Die beiden Dateien `PKGBUILD` und `.SRCINFO` in das AUR-Repository übernehmen:

```bash
git clone ssh://aur@aur.archlinux.org/klickmeister-bin.git aur-klickmeister-bin
cp klickmeister-bin/PKGBUILD klickmeister-bin/.SRCINFO aur-klickmeister-bin/
cd aur-klickmeister-bin
git add PKGBUILD .SRCINFO
git commit -m 'Update Klickmeister'
git push
```

Die Schritte gelten für die erste Einreichung eines freien Paketnamens oder für ein Paket, das du betreust. Nur `PKGBUILD` und `.SRCINFO` einchecken, keine Binärdateien oder Archive. Das Quellpaket `klickmeister` lässt sich entsprechend mit dem anderen AUR-Archiv pflegen.

`klickmeister-bin` stellt `klickmeister` bereit und steht mit ihm in Konflikt. Ein Wechsel erfolgt über den Paketmanager. Bei jeder neuen Version die Beschreibungen aus dem neuen Release übernehmen; Prüfsummen und Download-Adressen werden beim Build erzeugt.
