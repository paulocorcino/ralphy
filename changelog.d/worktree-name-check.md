---
kind: fix
---
The name field of the "New worktree" prompt now keeps out every character that
git does not accept in a branch name, such as a space or "/". Before, the
prompt sent the name and showed only "the request was not valid".
