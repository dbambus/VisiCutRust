# LTT iLaser 4000: Portierungsstand

Der Rust-Treiber basiert auf `LaserToolsTechnicsCutter.java` aus
LibLaserCut (Autor Maximilian Gaukler, mit Teilen von Thomas Oster,
LGPL-3.0-or-later). Die Originalquelle und das FAU-Geräteprofil stehen
im Ordner `reference`. Abruf: 8. Oktober 2026.

Originalquellen:

- https://github.com/t-oster/LibLaserCut/blob/master/src/main/java/de/thomas_oster/liblasercut/drivers/LaserToolsTechnicsCutter.java
- https://github.com/fau-fablab/visicut-settings/blob/master/devices/LTT_32_iLaser_32_4000.xml

Übernommen wurden Initialisierung, Dateiname mit VC-Präfix, XY-Modus,
Bounding-Box-Struktur, Prescaling 8 (4000/500 DPI), Materialradius,
Kompressionskonstante C0, Graustufenpalette, Vektormodus,
Leistungs- und Geschwindigkeitsbefehle, PPI-Teiler, PA/PR/PD/PU,
Rasterzeilen, abschließendes BYE, additive 16-Bit-Prüfsumme und
32-Bit-Dateilänge. Zahlen sind Big Endian. Der normale Y-Ursprung
wird für Bewegungen vom oberen zum unteren linken Rand umgerechnet.
Bounding-Box-Werte folgen der separaten Längen-Konvertierung des
Java-Treibers und werden dort nicht gespiegelt.

Das veröffentlichte FAU-Profil verwendet `lasercutter2:9100`,
1000 × 600 mm, 4000 Maschinen-DPI, 500 Job-DPI und 338,677 mm/s
nominale Schnittgeschwindigkeit. Der Faktor zwischen Raster- und
Schnittgeschwindigkeit ist 6,4 wie im Java-Treiber. Die Raster-Verschiebetabelle
ist [-2, -4, -7, -10, -13, -15, -17, -19, -21, -23].

Seit Version 0.2.1 ist die eingestellte Geschwindigkeit ein Prozentwert
von 0,1 bis 100 %, wie im ursprünglichen LTT-Treiber und in den FAU-Profilen.
Der ESC-S-Befehl erhält diesen Wert in Zehntelprozent: 9 % wird zu 90,
100 % zu 1000. Die zuvor verwendete Umrechnung in mm/s ist ausschließlich
für die Migration von Projektformat 1 erhalten. Neue Projekte speichern
`speed_percent` in Formatversion 2; die Migration berücksichtigt Schnitt
und Gravur getrennt, damit bestehende Einstellungen erhalten bleiben.

Die Rust-App schickt Jobs ohne Autorun über eine TCP-Verbindung, deren
Schreibseite anschließend geschlossen wird. Das Protokoll enthält hier
keine Gerätequittung; erfolgreiche TCP-Übertragung bestätigt keine
erfolgreiche Verarbeitung auf dem Lasercutter. Es gibt keine automatische
Wiederholung nach Fehlern.

## Bewusste Einschränkungen

- Vektorkurven werden adaptiv in Geraden zerlegt (0,025 mm Toleranz),
  anschließend auf 500 DPI quantisiert. Tangentialkurven, Kreisbefehle
  und deren Geschwindigkeitsoptimierung sind noch nicht portiert.
- SVG-Objekte können einem Schnitt-, Markier- oder Gravurschritt zugeordnet oder ignoriert
  werden. Jeder Schritt hat eigene Leistung, Geschwindigkeit und Durchgänge.
  Pro Verfahren wird ein eigenständig gerahmter LTT-Job erzeugt, maximal drei,
  in der Reihenfolge Engrav → Mark → Cut, mit jeweils eigener TCP-Verbindung.
  Markieren verwendet Vektorkonturen mit eigenen Parametern. Automatische
  Farbzuordnungen und mehrere Parametersätze je Verfahren fehlen.
  Text muss vor dem Schneiden in Pfade umgewandelt werden; Rasterbilder,
  Masken, Clipping und Filter werden beim Schneiden abgewiesen.
- Rastergravur ist Schwarz/Weiß mit Luminanzschwelle 128, 500 DPI,
  links nach rechts. Overscan und Verschiebung werden berücksichtigt.
  Graustufengravur, bidirektionale Gravur und Drehachse fehlen.
- Externe SVG-Bilder werden nicht geladen; Bilder müssen eingebettet sein.
- Autofokus, Druckluft und Absaugung werden wie im FAU-Profil nicht
  vom Treiber gesteuert.
- SVG und `.vcr` werden unterstützt. Version 0.2 ergänzt eine native
  AppKit-/SwiftUI-Oberfläche und eine Auswahl der FAU-LTT-Materialprofile.
  Bestehende VisiCut-PLF-Dateien, DXF/EPS, allgemeiner Materialbibliothek-
  Import, Kamera und andere Gerätetreiber fehlen.

Dies ist ein ausführbarer Anfang der Portierung mit durchgehendem
SVG→Rust→LTT-Workflow, keine vollständige Funktionsparität mit VisiCut.
Vor Produktionseinsatz ist ein beaufsichtigter Vergleich mit dem
Java-Treiber am tatsächlichen Gerät erforderlich.

## Auftragsvorschau und Dauer

Die Vorschau entsteht aus den für den Job verwendeten Konturen und
Schwarz/Weiß-Rasterpixeln. Schnittkonturen erscheinen rot, Gravurpixel blau und Markierkonturen violett.
Die Schätzung verwendet die quantisierten Vektorwege und die tatsächlichen
Rasterzeilen einschließlich Overscan, nomineller Rückfahrt und Durchgängen.
Rasterzeilen erhalten wie im Java-Treiber einen Zuschlag von 0,1 Sekunden.
Die Geschwindigkeiten werden für die Schätzung wie für ESC-S auf Zehntelprozent
quantisiert. Nicht modelliert sind die exakten Beschleunigungsprofile,
Gerätestart, Übertragungszeit und Bedienpausen. Keine Kalibrierung am Gerät.

Projektformat 2 enthält optional `steps`: maximal je einen Schnitt-, Markier-
und Gravurschritt, mit nullbasierten SVG-Objektindizes und eigenen Parametern.
Ein fehlendes oder leeres `steps` behält die bisherige Ganzmotiv-Bearbeitung bei.
Leere Objektauswahlen innerhalb expliziter Schritte werden übersprungen;
bei vollständig leerem Auftrag, doppelten oder ungültigen Objektindizes wird
kein Job erzeugt. Ein neuer SVG-Import verwirft vorhandene Zuordnungen.

Der Zeitslider verwendet Bewegungen aus der Erzeugung der tatsächlichen
LTT-Daten. Jeder Vektorschritt wird nach 500-DPI-Quantisierung erfasst,
Rasterzeilen einschließlich Overscan und Leerfahrten. Wiederholte Durchgänge
teilen die gespeicherte Geometrie; Zeitabschnitte bilden die Reihenfolge ab.
Die erste Anfahrt wird ab (0,0) geschätzt; danach wird die letzte Position
fortgeschrieben. Tatsächliche Kopfposition und Pausen zwischen den getrennt
zu startenden Gerätejobs sind nicht bekannt. Die Simulation sendet keine Daten.

Gerätenamen beginnen direkt mit `Cut_`, `Engrav_` bzw. `Mark_`, ohne zusätzlichen
VC-Präfix, und werden auf 15 ASCII-Zeichen begrenzt. Der Präfix bleibt beim
Kürzen erhalten. Alle Teiljobs werden vor dem ersten Netzwerkzugriff geprüft.
Jeder hat eigenen Header, BYE, Prüfsumme, Länge und deaktivierten Autostart.
Bei einer fehlgeschlagenen Übertragung stoppt die Folge; die Fehlermeldung
nennt den betroffenen und bereits übertragenen Teil. Es gibt weder eine
Gerätequittung noch automatisches Fortsetzen oder eine automatische Wiederholung.
