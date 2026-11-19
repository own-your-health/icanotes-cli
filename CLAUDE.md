# CLAUDE.md

`AGENTS.md` is canonical; read it first. The rules that matter most:

- These are behavioral-health records. Never put a real record, document,
  name or patient id in a tracked file, commit, PR, issue or message to
  another session; report real checks as counts and exit codes.
- The key and secret travel in the query string: never log or print a
  request URL, and never pass a key in argv.
- If an `icanotes` call hangs, it waits on a Keychain or 1Password approval:
  stop calling it and tell the owner.
- No MCP surface: no `mcp` command and no cli-mcp registration.
