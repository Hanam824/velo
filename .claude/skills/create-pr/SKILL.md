---
name: create-pr
description: Push the current branch and open a pull request into main with auto-merge (squash) enabled. Use when the user asks to push, open or create a pull request.
disable-model-invocation: true
---

# Create pull request

Work reaches `main` only through pull requests; `CI result` is the required check.

## Steps

1. Check the starting point:
   ```bash
   git status --short
   git branch --show-current
   git log --oneline origin/main..HEAD
   ```
   On `main`, create a branch first (`create-commit-branch` skill). With uncommitted changes, ask whether to commit them (`create-commit-message` skill).

2. Run the `check` skill. If it fails, stop and report; don't open a pull request that is known to fail.

3. Push:
   ```bash
   git push -u origin <branch>
   ```

4. Open the pull request. The title is the commit header (`type(scope): description`, no `!`). The body:
   ```
   # Summary
   - <change 1>
   - <change 2>

   # BREAKING CHANGE
   - <what breaks for users or older installs>

   # Testing
   - <what was run, and on which OS>
   ```
   Leave out `# BREAKING CHANGE` when nothing breaks. Use a heredoc so line breaks survive:
   ```bash
   gh pr create --base main --title "<title>" --body "$(cat <<'MSG'
   <body>
   MSG
   )"
   ```

5. Enable auto-merge, so it merges when CI is green:
   ```bash
   gh pr merge --auto --squash
   ```

6. Give the user the pull request's address, and say that CI has only started.

## Rules

- **No AI attribution** in the title or body (no `Co-Authored-By`, no "Generated with"). A squash merge copies the body into the commit on `main`. A hook refuses it.
- Keep the base `main`. Never retarget a pull request onto another branch while auto-merge is on: it would merge into that branch. To build on an unmerged pull request, merge its branch into yours instead.
- If the pull request is `BEHIND` after another one merged, update it: `gh pr update-branch <number>`.
- A Dependabot pull request is rebased by commenting `@dependabot rebase`, not by pushing to its branch (Dependabot stops maintaining a branch someone else pushed to).
- Rewriting a pushed branch (rebase, amend) needs `git push --force-with-lease`, and only when the user asked for the rewrite.
