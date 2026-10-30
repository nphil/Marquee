#!/bin/sh
# Hand a television its Bazarr address and API key without typing them there.
#
#   tools/import-bazarr-link.sh SSH_HOST KEY_FILE [URL]      (URL defaults to https://bazarr.nateshome.net)
#   tools/import-bazarr-link.sh --dry-run KEY_FILE [URL]     (checks the file and the address, writes nothing)
#
# Builds {"url": ..., "api_key": ...} from KEY_FILE and writes it, over ssh's stdin, to
# /tmp/marquee-bazarr-import.json on SSH_HOST with mode 0600. The key is never on a command line, in an
# environment variable or in this script's output. The app takes the file ONCE at its next start (sign in
# first; a signed-out start leaves it for the next one), stores the link in its sealed session record
# and deletes the file: rust-modules/plex/src/plex/session/bazarr_import.rs. Restart the app after this.
set -eu

dry=0
if [ "${1:-}" = "--dry-run" ]; then dry=1; shift; fi
if [ "$dry" = 1 ]; then
  keyfile=${1:?usage: import-bazarr-link.sh --dry-run KEY_FILE [URL]}; url=${2:-https://bazarr.nateshome.net}
else
  host=${1:?usage: import-bazarr-link.sh SSH_HOST KEY_FILE [URL]}; keyfile=${2:?key file}; url=${3:-https://bazarr.nateshome.net}
fi
remote=/tmp/marquee-bazarr-import.json

[ -r "$keyfile" ] || { echo "import-bazarr-link: no readable key file" >&2; exit 1; }
case "$(stat -c %a "$keyfile")" in
  *00) ;;
  *) echo "import-bazarr-link: the key file is readable by others; tighten it first (chmod 600)" >&2; exit 1 ;;
esac

# The JSON goes to stdout only; the key is read from the file inside python, never passed as an argument.
payload() {
  python3 -c '
import json, sys
key = open(sys.argv[2]).read().strip()
if not key or any(c.isspace() or not c.isprintable() for c in key):
    sys.exit("import-bazarr-link: the key file does not hold one key")
sys.stdout.write(json.dumps({"url": sys.argv[1], "api_key": key}))
' "$url" "$keyfile"
}

if [ "$dry" = 1 ]; then
  bytes=$(payload | wc -c)
  echo "import-bazarr-link: ok, $bytes bytes would be written to $remote"
  exit 0
fi

payload | ${SSH:-ssh} "$host" "umask 077; cat > $remote && chmod 600 $remote"
echo "import-bazarr-link: wrote $remote on $host; restart the app to take it"
