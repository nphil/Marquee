#!/bin/sh
# Exercise the actual publisher against a local Wayland server, without an ARM build.
set -eu
cd "$(dirname "$0")/.."
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
cc -std=c11 -Wall -Wextra -Werror -c ci/subtitle_wayland_smoke.c -o "$tmp/server.o"
rustc --edition=2021 ci/subtitle_wayland_smoke.rs -C "link-arg=$tmp/server.o" \
    -l wayland-client -l wayland-server -l pthread -l dl -o "$tmp/smoke"
"$tmp/smoke"
