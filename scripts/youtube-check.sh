#!/usr/bin/env bash
# Check, from this machine, that Velo can still download real YouTube videos: for a short and a
# long video it asks every client for the start, the middle and a 10 MiB range of the best
# stream, and fails if streams are served at the start but refused deeper.
#
#   ./scripts/youtube-check.sh                      # the default long video
#   VELO_LIVE_LONG_VIDEO=<id or address> ./scripts/youtube-check.sh
#
# Run it from a home connection (by hand, or from cron or launchd): YouTube refuses every
# stream to datacenter addresses such as GitHub's runners, so the nightly workflow can't do
# this part and says "SKIPPED" instead.
set -euo pipefail

cd "$(dirname "$0")/.."

VELO_LIVE_YOUTUBE=1 cargo test -p velo-extract --test live --locked -- --ignored --nocapture
