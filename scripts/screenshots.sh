#!/bin/sh
# The screenshots of the README, in assets/screenshots: the dashboard on the demo database, 7 days
# of a simulated market traded by the strategy of the bot. To run before a release, and whenever the
# screens change:
#
#   scripts/screenshots.sh [FOLDER]
set -eu

folder="${1:-assets/screenshots}"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cargo run --quiet -p kraken-dashboard --example demo -- "$work/demo.sqlite"
cargo run --quiet -p kraken-dashboard --example screenshots -- "$work/demo.sqlite" "$folder"
