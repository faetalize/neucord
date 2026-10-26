#!/usr/bin/env bash
# The weekly job: snapshot the current client, compare it with the last
# snapshot (kept on the discord-snapshots branch, so master stays clean),
# store the new one there, and open an issue if anything Neucord uses
# changed. DRY_RUN=1 prints the report instead of pushing or filing.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(git -C "$here" rev-parse --show-toplevel)
channel=${CHANNEL:-stable}
branch=discord-snapshots
work=$(mktemp -d)
trap 'git -C "$repo" worktree remove --force "$work/snapshots" 2>/dev/null || true; rm -rf "$work"' EXIT
cli() { node "$here/cli.mjs" "$@"; }

cli fetch "$work/build" --channel "$channel" >/dev/null
cli snapshot "$work/build" --out "$work/new.json"
build=$(node -p 'require(process.argv[1]).build' "$work/new.json")

# A dry run leaves no branch behind: detached, or no checkout at all.
if git -C "$repo" fetch -q origin "$branch" 2>/dev/null; then
  if [[ ${DRY_RUN:-} == 1 ]]; then
    git -C "$repo" worktree add -q --detach "$work/snapshots" "origin/$branch"
  else
    git -C "$repo" worktree add -q -B "$branch" "$work/snapshots" "origin/$branch"
  fi
elif [[ ${DRY_RUN:-} == 1 ]]; then
  mkdir "$work/snapshots"
else
  git -C "$repo" worktree add -q --orphan -b "$branch" "$work/snapshots"
fi
# BASELINE=<file> compares against a snapshot of your choosing (testing).
old=${BASELINE:-$work/snapshots/$channel.json}

affects=0
if [[ -f $old ]]; then
  if [[ $(node -p 'require(process.argv[1]).build' "$old") == "$build" ]]; then
    echo "$channel build $build: already recorded"
    exit 0
  fi
  counts=$(cli compare "$old" "$work/new.json" --repo "$repo" --report "$work/report.md")
  affects=$(node -p 'JSON.parse(process.argv[1]).affects' "$counts")
  echo "$channel build $build: $counts"
else
  # Nothing to compare yet, but still check that every call is recognised.
  cli compare "$work/new.json" "$work/new.json" --repo "$repo" >/dev/null
  echo "$channel build $build: first snapshot, nothing to compare"
fi

if [[ ${DRY_RUN:-} == 1 ]]; then
  [[ -f $work/report.md ]] && cat "$work/report.md"
  exit 0
fi

cp "$work/new.json" "$old"
git -C "$work/snapshots" add "$channel.json"
git -C "$work/snapshots" -c user.name="github-actions[bot]" \
  -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
  commit -q -m "$channel build $build"
git -C "$work/snapshots" push -q origin "$branch"

if (( affects > 0 )); then
  commit=$(git -C "$work/snapshots" rev-parse HEAD)
  printf '\nFull snapshot diff: %s/%s/commit/%s\n' "$GITHUB_SERVER_URL" "$GITHUB_REPOSITORY" "$commit" >>"$work/report.md"
  gh label create discord-watch --color 5865F2 --description "Changes in Discord's official client" --force >/dev/null
  gh issue create --title "Discord $channel build $build changes what Neucord uses" \
    --label discord-watch --body-file "$work/report.md"
fi
