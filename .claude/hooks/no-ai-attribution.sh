#!/usr/bin/env bash
# PreToolUse hook for Bash: refuses a commit or pull request that carries AI attribution
# (CLAUDE.md: "Never add Co-Authored-By or other AI attribution").
# Reads the hook's JSON from stdin and greps it as text, so it needs no jq.
input=$(cat)
if printf '%s' "$input" | grep -qE 'git commit|gh pr (create|edit)' &&
    printf '%s' "$input" | grep -qiE 'co-authored-by|generated with[^a-z]*claude'; then
    echo "velo: no Co-Authored-By or other AI attribution in commits and pull requests (CLAUDE.md). Remove it and try again." >&2
    exit 2
fi
exit 0
