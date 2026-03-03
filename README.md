# brew

A terminal email client for Maildir folders, written in Rust.

## Features

- Threaded email list with tree indentation
- Diff/patch syntax highlighting in email bodies
- Reply-all and blank-reply composition in `$VISUAL`/`$EDITOR`
- Pre-written "thanks" replies
- Read/unread tracking via Maildir filename flags (no external state file)
- Filesystem watcher — thread list refreshes automatically on new mail
- Multiple mailboxes, switchable from the list view
- SMTP sending via STARTTLS (Gmail App Passwords supported)
- Periodic background sync via a configurable shell command (e.g. `mbsync`)

## Configuration

Create `~/.config/brew/config.toml`:

```toml
[smtp]
host     = "smtp.gmail.com"
port     = 587
name     = "Name Surname"
username = "you@example.com"
password = "app-password"

[[mailbox]]
label = "INBOX"
path  = "/home/you/Mail/INBOX/"

[[mailbox]]
label = "Work"
path  = "/home/you/Mail/Work/"

# Optional: run a shell command every N seconds to sync mail.
# Errors are shown in the status bar; the UI is never blocked.
[sync]
command  = "mbsync -a"
interval = 60
```

Optional files (plain text, loaded from `~/.config/brew/`):

| File        | Purpose                                  |
| ----------- | ---------------------------------------- |
| `signature` | Appended to every reply draft            |
| `thanks`    | Body pre-filled for `t` (thanks) replies |

## Key bindings

### List view

| Key                   | Action                         |
| --------------------- | ------------------------------ |
| `j` / `k`             | Move down / up                 |
| `g`                   | Jump to top                    |
| `G`                   | Jump to bottom                 |
| `PageDown` / `Ctrl+D` | Skip 15 down                   |
| `PageUp` / `Ctrl+U`   | Skip 15 up                     |
| `J` / `K`             | Next / previous mailbox        |
| `Enter`               | Open email                     |
| `r`                   | Reply-all (quoted)             |
| `R`                   | Reply-all (blank)              |
| `t`                   | Send thanks reply              |
| `C`                   | Compose new email              |
| `N`                   | Show unread only               |
| `n`                   | Show all emails                |
| `A`                   | Mark all emails as read        |
| `/`                   | Search by subject              |
| `\`                   | Search by sender               |
| `F`                   | Reset view (clear all filters) |
| `f`                   | Force sync mailboxes           |
| `D`                   | Delete email                   |
| `h` / `l`             | Switch tab left / right        |
| `Q`                   | Quit                           |

### Email view

| Key                   | Action                           |
| --------------------- | -------------------------------- |
| `j` / `k`             | Scroll down / up one line        |
| `PageDown` / `Ctrl+D` | Scroll 15 lines down             |
| `PageUp` / `Ctrl+U`   | Scroll 15 lines up               |
| `g`                   | Top of body                      |
| `G`                   | Bottom of body                   |
| `J` / `K`             | Next / previous email (filtered) |
| `r`                   | Reply-all (quoted)               |
| `R`                   | Reply-all (blank)                |
| `t`                   | Send thanks reply                |
| `D`                   | Close tab and delete email       |
| `h` / `l`             | Switch tab left / right          |
| `Esc`                 | Back to list                     |
| `q`                   | Close tab                        |

### Global

| Key      | Action              |
| -------- | ------------------- |
| `Ctrl+N` | Next tab (circular) |
| `Ctrl+P` | Prev tab (circular) |
