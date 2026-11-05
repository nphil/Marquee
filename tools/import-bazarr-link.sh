#!/bin/sh
# Hand a television its Bazarr address and API key without typing them there.
#
#   tools/import-bazarr-link.sh SSH_HOST KEY_FILE [URL]      (URL defaults to https://bazarr.nateshome.net)
#   tools/import-bazarr-link.sh --dry-run KEY_FILE [URL]     (checks the file and the address, writes nothing)
#
# Builds {"url": ..., "api_key": ...} from KEY_FILE and writes it, over ssh's stdin, to
# /tmp/marquee-bazarr-import.json on SSH_HOST, mode 0600, owned by the uid and group of the app's jail
# directory (/var/palm/jail/com.beb.plxnative, the owner of every file the app creates). The app runs
# jailed under that uid, not root, and refuses a file it does not own, so a root-owned file would be
# deleted unread. The key is never on a command line, in an environment variable or in this script's
# output. The app takes the file ONCE at its next start (sign in first; a signed-out start leaves it for
# the next one), stores the link in its sealed session record and deletes the file:
# rust-modules/plex/src/plex/session/bazarr_import.rs. Restart the app after this.
set -eu

dry=0
if [ "${1:-}" = "--dry-run" ]; then dry=1; shift; fi
if [ "$dry" = 1 ]; then
  keyfile=${1:?usage: import-bazarr-link.sh --dry-run KEY_FILE [URL]}; url=${2:-https://bazarr.nateshome.net}
else
  host=${1:?usage: import-bazarr-link.sh SSH_HOST KEY_FILE [URL]}; keyfile=${2:?key file}; url=${3:-https://bazarr.nateshome.net}
fi
remote=/tmp/marquee-bazarr-import.json
jail=/var/palm/jail/com.beb.plxnative

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

# The owner is read off the jail directory on the set, never hard-coded: the uid is assigned at install.
# `rm -f` first so a planted link or a stale file of another owner is replaced, not written through.
# A failed stat or chown removes the file rather than leaving a key behind.
payload | ${SSH:-ssh} "$host" "umask 077; rm -f $remote; cat > $remote && \
own=\$(stat -c %u:%g $jail) && chown \"\$own\" $remote && chmod 600 $remote || { rm -f $remote; exit 1; }"
echo "import-bazarr-link: wrote $remote on $host; restart the app to take it"
