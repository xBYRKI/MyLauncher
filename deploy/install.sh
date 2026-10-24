#!/bin/sh
# Installiert bzw. aktualisiert mylauncher-server auf dem Server. Läuft als root,
# aufgerufen von scripts/deploy-server.ps1 aus dem Ordner, in den es die Dateien kopiert hat:
#
#   mylauncher-server        Programm (Linux, statisch gelinkt)
#   server.env               Umgebung für den Dienst (Admin-Passwort, öffentliche URL …)
#   mylauncher-server.service
#   mylauncher.caddy         optional: Site-Block für Caddy
set -eu

SRC=$(cd "$(dirname "$0")" && pwd)
BIN=/opt/mylauncher
DATA=/var/lib/mylauncher
ETC=/etc/mylauncher

if [ "$(id -u)" -ne 0 ]; then
    echo "install.sh muss als root laufen" >&2
    exit 1
fi
# Der Upload-Ordner enthält das Admin-Passwort – in jedem Fall wieder entfernen.
trap 'rm -rf "$SRC"' EXIT

if ! id mylauncher >/dev/null 2>&1; then
    useradd --system --home-dir "$DATA" --shell /usr/sbin/nologin mylauncher
    echo "Benutzer mylauncher angelegt"
fi
install -d -o mylauncher -g mylauncher -m 750 "$DATA"
install -d -m 755 "$BIN"
install -d -m 750 "$ETC"

# Erst daneben ablegen, dann umbenennen: die laufende Datei wird nicht überschrieben.
install -m 755 "$SRC/mylauncher-server" "$BIN/mylauncher-server.new"
mv -f "$BIN/mylauncher-server.new" "$BIN/mylauncher-server"
install -m 600 "$SRC/server.env" "$ETC/server.env"
install -m 644 "$SRC/mylauncher-server.service" /etc/systemd/system/mylauncher-server.service

systemctl daemon-reload
systemctl enable --quiet mylauncher-server
systemctl restart mylauncher-server

if [ -f "$SRC/mylauncher.caddy" ]; then
    if command -v caddy >/dev/null 2>&1 && [ -f /etc/caddy/Caddyfile ]; then
        install -m 644 "$SRC/mylauncher.caddy" /etc/caddy/mylauncher.caddy
        if ! grep -q '^import /etc/caddy/mylauncher.caddy' /etc/caddy/Caddyfile; then
            printf '\nimport /etc/caddy/mylauncher.caddy\n' >> /etc/caddy/Caddyfile
            echo "Caddyfile: import /etc/caddy/mylauncher.caddy ergänzt"
        fi
        caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
        systemctl reload caddy
        echo "Caddy neu geladen"
    else
        echo "WARNUNG: Caddy nicht gefunden – Reverse-Proxy selbst einrichten:" >&2
        cat "$SRC/mylauncher.caddy" >&2
    fi
fi

for _ in 1 2 3 4 5 6 7 8 9 10; do
    if systemctl is-active --quiet mylauncher-server; then
        echo "mylauncher-server läuft ($("$BIN/mylauncher-server" --version 2>/dev/null || echo installiert))"
        exit 0
    fi
    sleep 1
done
journalctl -u mylauncher-server -n 30 --no-pager >&2
exit 1
